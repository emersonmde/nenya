//! HTTP API for rate limiting requests
//!
//! Implements the REST API:
//! - POST /should_throttle
//! - GET /scope_stats?scope=<name>
//! - GET /health
//! - GET /metrics

#[cfg(feature = "server")]
pub mod error;

#[cfg(feature = "server")]
pub mod handlers;

#[cfg(feature = "server")]
pub mod manager;

#[cfg(feature = "server")]
pub mod metrics;

#[cfg(feature = "server")]
pub use error::ApiError;

#[cfg(feature = "server")]
pub use handlers::{AppState, HealthResponse, ThrottleRequest, ThrottleResponse};

#[cfg(feature = "server")]
pub use manager::{RateLimitManager, ScopePattern, ScopeStats, ThrottleDecision};

#[cfg(feature = "server")]
pub use metrics::MetricsCollector;

#[cfg(feature = "server")]
use axum::{
    routing::{get, post},
    Router,
};

#[cfg(feature = "server")]
use std::sync::Arc;

/// Create the HTTP router with all endpoints
#[cfg(feature = "server")]
pub fn create_router(state: Arc<handlers::AppState>) -> Router {
    Router::new()
        .route("/should_throttle", post(handlers::should_throttle))
        .route("/scope_stats", get(handlers::scope_stats))
        .route("/health", get(handlers::health))
        .route("/metrics", get(handlers::metrics))
        .with_state(state)
}
