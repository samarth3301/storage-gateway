use axum::{
    body::Body,
    http::{HeaderName, Request},
    middleware::Next,
    response::Response,
    routing::{get, post},
    Router,
};
use moka::future::Cache;
use mongodb::Database;
use std::time::{Duration, Instant};
use tower::limit::ConcurrencyLimitLayer;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tracing::info;

use crate::config::Config;
use crate::handlers::{
    get_object, health, list_buckets, liveness, metrics, readiness, register_bucket, reset_cache,
};
use crate::models::{BucketClient, CachedObject};
use crate::services::crypto::Cipher;
use crate::services::{cache, db, storage};

const MAX_IN_FLIGHT_REQUESTS: usize = 1024;
const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

#[derive(Clone)]
pub struct AppState {
    pub s3_clients: Cache<String, BucketClient>,
    pub cache: Cache<String, CachedObject>,
    pub not_found: Cache<String, ()>,
    pub max_cached_object_bytes: u64,
    pub cipher: Cipher,
    pub db: Database,
    pub registration_token: String,
    pub start_time: Instant,
}

impl AppState {
    pub async fn init(config: &Config) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let cipher = Cipher::from_base64_key(&config.auth.credentials_key)?;
        let db = db::connect(&config.database).await?;

        let s3_clients = Cache::builder()
            .max_capacity(1000)
            .time_to_live(Duration::from_secs(3600))
            .build();
        storage::preload_clients(&db, &s3_clients, &cipher).await?;

        Ok(Self {
            s3_clients,
            cache: cache::object_cache(&config.cache),
            not_found: cache::not_found_cache(&config.cache),
            max_cached_object_bytes: config.cache.max_object_bytes,
            cipher,
            db,
            registration_token: config.auth.registration_token.clone(),
            start_time: Instant::now(),
        })
    }
}

async fn access_log(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let request_id = req
        .headers()
        .get(&REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let start = Instant::now();

    let response = next.run(req).await;

    let elapsed = start.elapsed();
    let status = response.status().as_u16();
    metrics::histogram!(
        "http_request_duration_seconds",
        "method" => method.to_string(),
        "status" => status.to_string()
    )
    .record(elapsed.as_secs_f64());
    info!(
        method = %method,
        path = %path,
        status,
        duration_ms = elapsed.as_millis() as u64,
        request_id = %request_id,
        "HTTP Request"
    );
    response
}

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness))
        .route("/metrics", get(metrics))
        .route("/buckets", get(list_buckets))
        .route("/buckets/register", post(register_bucket))
        .route("/reset-cache", post(reset_cache))
        .route("/:instance/*key", get(get_object))
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::new(REQUEST_ID, MakeRequestUuid))
                .layer(PropagateRequestIdLayer::new(REQUEST_ID))
                .layer(axum::middleware::from_fn(access_log))
                .layer(CorsLayer::permissive())
                .layer(ConcurrencyLimitLayer::new(MAX_IN_FLIGHT_REQUESTS)),
        )
        .with_state(state)
}
