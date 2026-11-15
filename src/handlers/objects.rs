use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::operation::get_object::{GetObjectError, GetObjectOutput};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::{Bytes, BytesMut};
use metrics::counter;
use mongodb::bson::doc;
use std::io;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, warn};

use crate::app::AppState;
use crate::error::AppError;
use crate::handlers::admin::not_found_instance_key;
use crate::models::{BucketClient, CachedObject};
use crate::services::crypto::CryptoError;
use crate::services::db::buckets;
use crate::services::storage::create_s3_client;

const PUBLIC_CACHE_CONTROL: HeaderValue = HeaderValue::from_static("public, max-age=3600");
const OCTET_STREAM: HeaderValue = HeaderValue::from_static("application/octet-stream");
const ACCEPT_BYTES: HeaderValue = HeaderValue::from_static("bytes");
const MAX_KEY_LEN: usize = 1024;

#[derive(Debug, PartialEq)]
enum ByteRange {
    Full,
    Partial(u64, u64),
    Unsatisfiable,
}

#[tracing::instrument(skip(state, headers))]
pub async fn get_object(
    State(state): State<AppState>,
    Path((instance, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let key = validate_key(&key)?;
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let cache_key = format!("{instance}:{key}");

    if let Some(cached) = state.cache.get(&cache_key).await {
        counter!("cdn_cache_requests_total", "result" => "hit").increment(1);
        debug!(size = cached.data.len(), "Cache hit");
        return Ok(cached_response(cached, &headers, range));
    }
    if state.not_found.contains_key(&cache_key) {
        counter!("cdn_cache_requests_total", "result" => "negative_hit").increment(1);
        return Err(file_not_found(key));
    }
    counter!("cdn_cache_requests_total", "result" => "miss").increment(1);

    let bucket = bucket_client(&state, &instance).await?;
    let mut request = bucket.client.get_object().bucket(&bucket.bucket).key(key);
    if let Some(range) = range {
        request = request.range(range);
    }

    match request.send().await {
        Ok(output) => {
            // ponytail: ranged misses are proxied but not cached; prefetch the full object if video seeking dominates.
            let cache_key = range.is_none().then_some(cache_key);
            Ok(stream_response(output, cache_key, &state))
        }
        Err(e) => {
            if is_no_such_key(&e) {
                state.not_found.insert(cache_key, ()).await;
            }
            Err(s3_error(key, e))
        }
    }
}

fn validate_key(key: &str) -> Result<&str, AppError> {
    let key = key.trim_start_matches('/');
    if key.is_empty() {
        return Err(AppError::bad_request("Key cannot be empty", key));
    }
    if key.contains("..") {
        warn!("Rejected traversal key: {}", key);
        return Err(AppError::bad_request("Invalid key format", key));
    }
    if key.len() > MAX_KEY_LEN {
        return Err(AppError::bad_request("Key is too long", ""));
    }
    Ok(key)
}

fn file_not_found(key: &str) -> AppError {
    AppError::not_found("The requested file was not found", key)
}

fn parse_range(spec: &str, len: u64) -> ByteRange {
    let Some(spec) = spec.trim().strip_prefix("bytes=") else {
        return ByteRange::Full;
    };
    if spec.contains(',') {
        return ByteRange::Full;
    }
    let Some((start, end)) = spec.split_once('-') else {
        return ByteRange::Full;
    };
    let last = len.saturating_sub(1);
    let (start, end) = match (start.trim().parse::<u64>(), end.trim().parse::<u64>()) {
        (Ok(s), Ok(e)) if s <= e => (s, e.min(last)),
        (Ok(s), Err(_)) if end.trim().is_empty() => (s, last),
        (Err(_), Ok(n)) if start.trim().is_empty() && n > 0 => (len.saturating_sub(n), last),
        _ => return ByteRange::Full,
    };
    if len == 0 || start >= len {
        ByteRange::Unsatisfiable
    } else {
        ByteRange::Partial(start, end)
    }
}

fn etag_matches(request: &HeaderMap, etag: &HeaderValue) -> bool {
    let Some(Ok(candidates)) = request.get(header::IF_NONE_MATCH).map(HeaderValue::to_str) else {
        return false;
    };
    let tag = etag.to_str().unwrap_or_default().trim_start_matches("W/");
    candidates
        .split(',')
        .map(str::trim)
        .any(|c| c == "*" || c.trim_start_matches("W/") == tag)
}

fn base_headers(etag: Option<&HeaderValue>) -> HeaderMap {
    let mut headers = HeaderMap::with_capacity(6);
    headers.insert(header::CACHE_CONTROL, PUBLIC_CACHE_CONTROL);
    headers.insert(header::ACCEPT_RANGES, ACCEPT_BYTES);
    if let Some(etag) = etag {
        headers.insert(header::ETAG, etag.clone());
    }
    headers
}

fn content_range(value: String) -> HeaderValue {
    HeaderValue::from_str(&value).expect("content range is ASCII")
}

fn cached_response(cached: CachedObject, request: &HeaderMap, range: Option<&str>) -> Response {
    let mut headers = base_headers(cached.etag.as_ref());
    if cached
        .etag
        .as_ref()
        .is_some_and(|etag| etag_matches(request, etag))
    {
        return (StatusCode::NOT_MODIFIED, headers).into_response();
    }
    headers.insert(header::CONTENT_TYPE, cached.content_type);

    let total = cached.data.len() as u64;
    match range.map_or(ByteRange::Full, |r| parse_range(r, total)) {
        ByteRange::Full => (headers, cached.data).into_response(),
        ByteRange::Partial(start, end) => {
            headers.insert(
                header::CONTENT_RANGE,
                content_range(format!("bytes {start}-{end}/{total}")),
            );
            let body = cached.data.slice(start as usize..=end as usize);
            (StatusCode::PARTIAL_CONTENT, headers, body).into_response()
        }
        ByteRange::Unsatisfiable => {
            headers.insert(
                header::CONTENT_RANGE,
                content_range(format!("bytes */{total}")),
            );
            (StatusCode::RANGE_NOT_SATISFIABLE, headers).into_response()
        }
    }
}

enum LookupError {
    NotFound,
    Db(mongodb::error::Error),
    Crypto(CryptoError),
}

async fn bucket_client(state: &AppState, instance: &str) -> Result<BucketClient, AppError> {
    let missing_key = not_found_instance_key(instance);
    if state.not_found.contains_key(&missing_key) {
        return Err(instance_not_found(instance));
    }

    let db = state.db.clone();
    let cipher = state.cipher.clone();
    let name = instance.to_string();
    let lookup = state
        .s3_clients
        .try_get_with(name.clone(), async move {
            match buckets(&db).find_one(doc! {"name": &name}, None).await {
                Ok(Some(creds)) => create_s3_client(&creds, &cipher).map_err(LookupError::Crypto),
                Ok(None) => Err(LookupError::NotFound),
                Err(e) => Err(LookupError::Db(e)),
            }
        })
        .await;

    match lookup {
        Ok(client) => Ok(client),
        Err(e) => Err(match e.as_ref() {
            LookupError::NotFound => {
                state.not_found.insert(missing_key, ()).await;
                instance_not_found(instance)
            }
            LookupError::Db(err) => {
                error!("Database error fetching instance {}: {}", instance, err);
                AppError::internal("Database error", instance)
            }
            LookupError::Crypto(err) => {
                error!("Cannot decrypt credentials for {}: {}", instance, err);
                AppError::internal("Storage credentials unavailable", instance)
            }
        }),
    }
}

fn instance_not_found(instance: &str) -> AppError {
    AppError::not_found(format!("Instance '{instance}' not found"), instance)
}

fn stream_response(
    output: GetObjectOutput,
    cache_key: Option<String>,
    state: &AppState,
) -> Response {
    let content_type = output
        .content_type()
        .and_then(|v| HeaderValue::from_str(v).ok())
        .unwrap_or(OCTET_STREAM);
    let etag = output.e_tag().and_then(|v| HeaderValue::from_str(v).ok());
    let content_length = output.content_length().and_then(|l| u64::try_from(l).ok());
    let partial = output
        .content_range()
        .and_then(|v| HeaderValue::from_str(v).ok());
    let max_cached = state.max_cached_object_bytes;

    let mut headers = base_headers(etag.as_ref());
    headers.insert(header::CONTENT_TYPE, content_type.clone());
    if let Some(len) = content_length {
        headers.insert(header::CONTENT_LENGTH, len.into());
    }
    let status = match partial {
        Some(range) => {
            headers.insert(header::CONTENT_RANGE, range);
            StatusCode::PARTIAL_CONTENT
        }
        None => StatusCode::OK,
    };

    let mut buffer = match (&cache_key, content_length) {
        (None, _) => None,
        (Some(_), Some(len)) if len > max_cached => None,
        (Some(_), Some(len)) => Some(BytesMut::with_capacity(len as usize)),
        (Some(_), None) => Some(BytesMut::new()),
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, io::Error>>(16);
    let cache = state.cache.clone();
    let mut body = output.body;

    tokio::spawn(async move {
        let mut client_connected = true;
        while let Some(chunk) = body.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    counter!("cdn_origin_errors_total", "kind" => "stream").increment(1);
                    error!(key = ?cache_key, "Error streaming from S3: {}", e);
                    let _ = tx.send(Err(io::Error::other(e))).await;
                    return;
                }
            };

            let overflow = buffer
                .as_ref()
                .is_some_and(|b| (b.len() + chunk.len()) as u64 > max_cached);
            if overflow {
                buffer = None;
            } else if let Some(b) = buffer.as_mut() {
                b.extend_from_slice(&chunk);
            }

            if client_connected && tx.send(Ok(chunk)).await.is_err() {
                client_connected = false;
            }
            // Keep downloading after a disconnect only if the object is still headed for the cache.
            if !client_connected && buffer.is_none() {
                return;
            }
        }

        if let (Some(data), Some(key)) = (buffer, cache_key) {
            let object = CachedObject {
                data: data.freeze(),
                content_type,
                etag,
            };
            cache.insert(key, object).await;
        }
    });

    (status, headers, Body::from_stream(ReceiverStream::new(rx))).into_response()
}

fn is_no_such_key(err: &SdkError<GetObjectError>) -> bool {
    matches!(err, SdkError::ServiceError(service) if service.err().is_no_such_key())
}

fn s3_error(key: &str, err: SdkError<GetObjectError>) -> AppError {
    if is_no_such_key(&err) {
        return file_not_found(key);
    }
    if let SdkError::ServiceError(service) = &err {
        let code = service.err().code().unwrap_or("Unknown");
        if code == "InvalidRange" {
            return AppError::new(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "Requested range not satisfiable",
                key,
            );
        }
        counter!("cdn_origin_errors_total", "kind" => "service").increment(1);
        error!(key, s3_code = code, error = ?err, "S3 service error");
        // ponytail: private CDN, echo the S3 code so a misconfigured bucket is diagnosable from the response.
        return AppError::upstream(format!("S3 error: {code}"), key);
    }

    counter!("cdn_origin_errors_total", "kind" => "transport").increment(1);
    error!(key, error = ?err, "S3 request failed");
    AppError::upstream("Origin storage is unavailable", key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn if_none_match(value: &'static str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::IF_NONE_MATCH, HeaderValue::from_static(value));
        h
    }

    #[test]
    fn etag_matching() {
        let etag = HeaderValue::from_static("\"abc\"");
        assert!(etag_matches(&if_none_match("\"abc\""), &etag));
        assert!(etag_matches(&if_none_match("\"x\", W/\"abc\""), &etag));
        assert!(etag_matches(&if_none_match("*"), &etag));
        assert!(!etag_matches(&if_none_match("\"other\""), &etag));
        assert!(!etag_matches(&HeaderMap::new(), &etag));
    }

    #[test]
    fn key_validation() {
        assert_eq!(validate_key("/img/a.png").ok(), Some("img/a.png"));
        assert!(validate_key("a/../b").is_err());
        assert!(validate_key("/").is_err());
        assert!(validate_key(&"k".repeat(MAX_KEY_LEN + 1)).is_err());
    }

    #[test]
    fn range_parsing() {
        use ByteRange::*;
        assert_eq!(parse_range("bytes=0-4", 10), Partial(0, 4));
        assert_eq!(parse_range("bytes=5-", 10), Partial(5, 9));
        assert_eq!(parse_range("bytes=-3", 10), Partial(7, 9));
        assert_eq!(parse_range("bytes=-30", 10), Partial(0, 9));
        assert_eq!(parse_range("bytes=8-100", 10), Partial(8, 9));
        assert_eq!(parse_range("bytes=10-", 10), Unsatisfiable);
        assert_eq!(parse_range("bytes=0-1,4-5", 10), Full);
        assert_eq!(parse_range("bytes=5-2", 10), Full);
        assert_eq!(parse_range("items=0-1", 10), Full);
    }
}
