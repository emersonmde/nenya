//! HTTP API handlers

use std::sync::Arc;

#[cfg(feature = "server")]
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

#[cfg(feature = "server")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "server")]
use tokio::sync::RwLock;

use super::error::ApiError;
use super::manager::RateLimitManager;
use super::metrics::MetricsCollector;

/// Application state shared across handlers
#[cfg(feature = "server")]
pub struct AppState {
    pub manager: Arc<RwLock<RateLimitManager>>,
    pub metrics: MetricsCollector,
}

/// Request to check if throttling should occur
#[cfg(feature = "server")]
#[derive(Debug, Deserialize)]
pub struct ThrottleRequest {
    /// Scope identifier for the request
    pub scope: String,
}

/// Response indicating throttle decision
#[cfg(feature = "server")]
#[derive(Debug, Serialize)]
pub struct ThrottleResponse {
    /// Whether the request should be throttled
    pub should_throttle: bool,

    /// Current local accepted rate for this scope (from rate limiter)
    pub local_accepted_rate: f64,

    /// Current refill rate (tokens per second)
    pub refill_rate: f64,

    /// Number of peers in cluster (0 if not distributed)
    pub num_peers: usize,

    /// Total request rate (TPS) - from metrics
    pub total_request_rate: f64,

    /// Accepted request rate (TPS) - from metrics
    pub accepted_request_rate: f64,

    /// Throttled request rate (TPS) - from metrics
    pub throttled_request_rate: f64,

    /// DEBUG: Target rate for this limiter
    pub target_rate: f64,

    /// DEBUG: External accepted rate from peers
    pub external_accepted_rate: f64,

    /// DEBUG: Min rate bound
    pub min_rate: f64,

    /// DEBUG: Max rate bound
    pub max_rate: f64,

    /// Coordination tier: `local`, `tail`, or `hot`
    pub tier: String,
}

// Removed From implementation - handler now constructs response directly

/// Health check response
#[cfg(feature = "server")]
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// Whether the service is healthy
    pub healthy: bool,

    /// Number of active scopes
    pub scopes: usize,

    /// Number of peers in cluster
    pub peers: usize,
}

/// POST /should_throttle - Check if a request should be throttled
#[cfg(feature = "server")]
pub async fn should_throttle(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ThrottleRequest>,
) -> Result<Json<ThrottleResponse>, ApiError> {
    if req.scope.is_empty() {
        return Err(ApiError::InvalidRequest(
            "scope cannot be empty".to_string(),
        ));
    }

    let mut manager = state.manager.write().await;
    let decision = manager.should_throttle(&req.scope);

    // Get debug info from the scope snapshot (works for every tier)
    let (target_rate, external_accepted_rate, min_rate, max_rate, tier) = {
        if let Some(snap) = manager.scope_snapshot(&req.scope, std::time::Instant::now()) {
            (
                snap.target_rate,
                snap.external_rate,
                50.0,  // TODO: Get from limiter (not exposed yet)
                200.0, // TODO: Get from limiter (not exposed yet)
                snap.tier.to_string(),
            )
        } else {
            (0.0, 0.0, 0.0, 0.0, "unknown".to_string())
        }
    };

    // Record metrics
    state
        .metrics
        .record_request(&req.scope, decision.should_throttle)
        .await;

    // Get request rates from metrics
    let total_request_rate = state.metrics.total_rate(&req.scope).await;
    let accepted_request_rate = state.metrics.accepted_rate(&req.scope).await;
    let throttled_request_rate = state.metrics.throttled_rate(&req.scope).await;

    let response = ThrottleResponse {
        should_throttle: decision.should_throttle,
        local_accepted_rate: decision.local_accepted_rate,
        refill_rate: decision.refill_rate,
        num_peers: decision.num_peers,
        total_request_rate,
        accepted_request_rate,
        throttled_request_rate,
        target_rate,
        external_accepted_rate,
        min_rate,
        max_rate,
        tier,
    };

    Ok(Json(response))
}

/// GET /health - Health check endpoint
#[cfg(feature = "server")]
pub async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let manager = state.manager.read().await;

    // Node-level live peer count, stamped by the sync loop each tick (no
    // longer derived per scope — tail scopes don't track peers themselves)
    let peers = manager.live_peers();

    Json(HealthResponse {
        healthy: true,
        scopes: manager.num_scopes(),
        peers,
    })
}

/// GET /metrics - Prometheus metrics endpoint
#[cfg(feature = "server")]
pub async fn metrics(State(state): State<Arc<AppState>>) -> Response {
    let body = state.metrics.export_prometheus().await;
    (StatusCode::OK, body).into_response()
}

/// GET /scope_stats?scope=<name> - Get scope statistics without recording a request
#[cfg(feature = "server")]
pub async fn scope_stats(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<ThrottleResponse>, ApiError> {
    let scope = params
        .get("scope")
        .ok_or_else(|| ApiError::InvalidRequest("missing 'scope' query parameter".to_string()))?;

    if scope.is_empty() {
        return Err(ApiError::InvalidRequest(
            "scope cannot be empty".to_string(),
        ));
    }

    let manager = state.manager.read().await;

    // Get stats without calling should_throttle (works for every tier)
    if let Some(snap) = manager.scope_snapshot(scope, std::time::Instant::now()) {
        // Get request rates from metrics
        let total_request_rate = state.metrics.total_rate(scope).await;
        let accepted_request_rate = state.metrics.accepted_rate(scope).await;
        let throttled_request_rate = state.metrics.throttled_rate(scope).await;

        let response = ThrottleResponse {
            should_throttle: false, // Not applicable for stats query
            local_accepted_rate: snap.local_accepted_rate,
            refill_rate: snap.refill_rate,
            num_peers: snap.num_peers,
            total_request_rate,
            accepted_request_rate,
            throttled_request_rate,
            target_rate: snap.target_rate,
            external_accepted_rate: snap.external_rate,
            min_rate: 50.0,  // TODO: Get from limiter
            max_rate: 200.0, // TODO: Get from limiter
            tier: snap.tier.to_string(),
        };

        Ok(Json(response))
    } else {
        // Scope doesn't exist yet
        Ok(Json(ThrottleResponse {
            should_throttle: false,
            local_accepted_rate: 0.0,
            refill_rate: 0.0,
            num_peers: 0,
            total_request_rate: 0.0,
            accepted_request_rate: 0.0,
            throttled_request_rate: 0.0,
            target_rate: 0.0,
            external_accepted_rate: 0.0,
            min_rate: 0.0,
            max_rate: 0.0,
            tier: "unknown".to_string(),
        }))
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::api::manager::RateLimitManager;

    fn create_test_state() -> Arc<AppState> {
        Arc::new(AppState {
            manager: Arc::new(RwLock::new(RateLimitManager::new(100.0, 0.8, 0.05, 0.04))),
            metrics: MetricsCollector::new(),
        })
    }

    #[tokio::test]
    async fn test_should_throttle_valid() {
        let state = create_test_state();

        let req = ThrottleRequest {
            scope: "test-scope".to_string(),
        };

        let result = should_throttle(State(state.clone()), Json(req)).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(!response.should_throttle);
        assert_eq!(response.num_peers, 0);
    }

    #[tokio::test]
    async fn test_should_throttle_empty_scope() {
        let state = create_test_state();

        let req = ThrottleRequest {
            scope: "".to_string(),
        };

        let result = should_throttle(State(state), Json(req)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_health_empty() {
        let state = create_test_state();

        let response = health(State(state)).await;
        assert!(response.healthy);
        assert_eq!(response.scopes, 0);
        assert_eq!(response.peers, 0);
    }

    #[tokio::test]
    async fn test_health_with_scopes() {
        let state = create_test_state();

        // Create a few scopes
        {
            let mut manager = state.manager.write().await;
            manager.should_throttle("scope1");
            manager.should_throttle("scope2");
        }

        let response = health(State(state)).await;
        assert!(response.healthy);
        assert_eq!(response.scopes, 2);
    }

    #[tokio::test]
    async fn test_metrics_export() {
        let state = create_test_state();

        // Generate some traffic
        {
            let mut manager = state.manager.write().await;
            manager.should_throttle("test");
            manager.should_throttle("test");
        }

        let response = metrics(State(state)).await;
        let (parts, _body) = response.into_parts();
        assert_eq!(parts.status, StatusCode::OK);
    }
}
