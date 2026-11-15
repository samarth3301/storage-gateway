use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    message: String,
    key: String,
}

impl AppError {
    pub fn new(status: StatusCode, message: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            key: key.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>, key: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message, key)
    }

    pub fn unauthorized(key: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "Invalid or missing API token",
            key,
        )
    }

    pub fn not_found(message: impl Into<String>, key: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message, key)
    }

    pub fn conflict(message: impl Into<String>, key: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message, key)
    }

    pub fn internal(message: impl Into<String>, key: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message, key)
    }

    pub fn upstream(message: impl Into<String>, key: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, message, key)
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'static str,
    message: &'a str,
    key: &'a str,
    timestamp: u64,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let error = match self.status {
            StatusCode::BAD_REQUEST => "bad_request",
            StatusCode::UNAUTHORIZED => "unauthorized",
            StatusCode::NOT_FOUND => "not_found",
            StatusCode::CONFLICT => "conflict",
            StatusCode::RANGE_NOT_SATISFIABLE => "range_not_satisfiable",
            StatusCode::BAD_GATEWAY => "upstream_error",
            _ => "internal_error",
        };
        let body = ErrorBody {
            error,
            message: &self.message,
            key: &self.key,
            timestamp: now_secs(),
        };
        (self.status, Json(body)).into_response()
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
