//! HTTP API error handling

#[cfg(feature = "server")]
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

#[cfg(feature = "server")]
use serde_json::json;

/// API errors
#[cfg(feature = "server")]
#[derive(Debug)]
pub enum ApiError {
    /// Invalid request format
    InvalidRequest(String),

    /// Internal server error
    Internal(String),
}

#[cfg(feature = "server")]
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::InvalidRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = Json(json!({
            "error": message,
        }));

        (status, body).into_response()
    }
}

#[cfg(feature = "server")]
impl From<String> for ApiError {
    fn from(msg: String) -> Self {
        ApiError::Internal(msg)
    }
}

#[cfg(feature = "server")]
impl From<&str> for ApiError {
    fn from(msg: &str) -> Self {
        ApiError::Internal(msg.to_string())
    }
}
