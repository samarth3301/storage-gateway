use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use futures::TryStreamExt;
use mongodb::bson::doc;
use mongodb::options::FindOptions;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::app::AppState;
use crate::error::{now_secs, AppError};
use crate::models::{BucketCredentials, BucketStatus};
use crate::services::db::{buckets, is_duplicate_key, BUCKETS};
use crate::services::storage::seal;

const TOKEN_HEADER: &str = "x-api-token";

#[derive(Serialize)]
struct SuccessResponse {
    status: &'static str,
    message: &'static str,
    timestamp: u64,
}

#[derive(Deserialize)]
pub struct RegisterBucketRequest {
    pub name: String,
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub bucket_name: String,
    pub region: String,
    #[serde(default)]
    pub force_path_style: bool,
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(given) = headers.get(TOKEN_HEADER) else {
        return false;
    };
    let (given, expected) = (given.as_bytes(), token.as_bytes());
    given.len() == expected.len()
        && given
            .iter()
            .zip(expected)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn require_token(headers: &HeaderMap, state: &AppState, scope: &str) -> Result<(), AppError> {
    if authorized(headers, &state.registration_token) {
        return Ok(());
    }
    warn!(scope, "Rejected request with invalid API token");
    Err(AppError::unauthorized(scope))
}

pub fn not_found_instance_key(instance: &str) -> String {
    format!("@instance:{instance}")
}

fn valid_instance_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub async fn reset_cache(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    require_token(&headers, &state, "reset-cache")?;

    state.cache.invalidate_all();
    state.not_found.invalidate_all();
    state.cache.run_pending_tasks().await;
    info!("Cache reset");

    Ok(Json(SuccessResponse {
        status: "success",
        message: "Cache has been reset",
        timestamp: now_secs(),
    }))
}

pub async fn list_buckets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    require_token(&headers, &state, "buckets")?;

    let options = FindOptions::builder()
        .projection(doc! {"_id": 0, "name": 1, "bucket_name": 1, "endpoint": 1, "region": 1})
        .sort(doc! {"name": 1})
        .build();
    let list_error = |e: mongodb::error::Error| {
        error!("Failed to list buckets: {}", e);
        AppError::internal("Failed to list buckets", "buckets")
    };
    let list: Vec<BucketStatus> = state
        .db
        .collection::<BucketStatus>(BUCKETS)
        .find(None, options)
        .await
        .map_err(list_error)?
        .try_collect()
        .await
        .map_err(list_error)?;

    Ok(Json(serde_json::json!({ "buckets": list })))
}

pub async fn register_bucket(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<RegisterBucketRequest>,
) -> Result<Response, AppError> {
    require_token(&headers, &state, "registration")?;
    if !valid_instance_name(&payload.name) {
        return Err(AppError::bad_request(
            "Name must be 1-64 characters of [A-Za-z0-9._-]",
            payload.name,
        ));
    }

    let mut bucket = BucketCredentials {
        id: None,
        name: payload.name,
        endpoint: payload.endpoint,
        access_key: payload.access_key,
        secret_key: payload.secret_key,
        bucket_name: payload.bucket_name,
        region: payload.region,
        force_path_style: payload.force_path_style,
    };
    seal(&mut bucket, &state.cipher);

    let result = buckets(&state.db)
        .insert_one(&bucket, None)
        .await
        .map_err(|e| {
            if is_duplicate_key(&e) {
                return AppError::conflict("Bucket name already registered", &bucket.name);
            }
            error!("Failed to store bucket in MongoDB: {}", e);
            AppError::internal("Failed to save bucket to database", &bucket.name)
        })?;

    state
        .not_found
        .invalidate(&not_found_instance_key(&bucket.name))
        .await;
    info!(name = %bucket.name, id = %result.inserted_id, "Bucket registered");
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "status": "success",
            "message": "Bucket registered successfully",
            "id": result.inserted_id
        })),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn compares_token() {
        let mut headers = HeaderMap::new();
        assert!(!authorized(&headers, "secret"));
        headers.insert(TOKEN_HEADER, HeaderValue::from_static("secret"));
        assert!(authorized(&headers, "secret"));
        assert!(!authorized(&headers, "secreT"));
        assert!(!authorized(&headers, "secret2"));
    }

    #[test]
    fn instance_names() {
        assert!(valid_instance_name("assets-bucket_1.v2"));
        assert!(!valid_instance_name("a/b"));
        assert!(!valid_instance_name(""));
        assert!(!valid_instance_name(&"a".repeat(65)));
    }
}
