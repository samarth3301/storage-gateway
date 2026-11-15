use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Json},
};
use mongodb::bson::doc;
use serde::Serialize;
use tracing::warn;

use crate::app::AppState;
use crate::error::now_secs;
use crate::telemetry::metrics_handle;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    database: &'static str,
    timestamp: u64,
    uptime_seconds: u64,
    cached_objects: u64,
    cached_bytes: u64,
}

pub async fn liveness() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

pub async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    health(State(state)).await
}

pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let database_up = match state.db.run_command(doc! {"ping": 1}, None).await {
        Ok(_) => true,
        Err(e) => {
            warn!("Database health check failed: {}", e);
            false
        }
    };

    let body = HealthResponse {
        status: if database_up { "healthy" } else { "degraded" },
        database: if database_up { "up" } else { "down" },
        timestamp: now_secs(),
        uptime_seconds: state.start_time.elapsed().as_secs(),
        cached_objects: state.cache.entry_count(),
        cached_bytes: state.cache.weighted_size(),
    };
    let status = if database_up {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body))
}

pub async fn metrics() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        metrics_handle().render(),
    )
}
