//! Metrics collection and Prometheus export

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

#[cfg(feature = "server")]
use tokio::sync::RwLock;

const RATE_WINDOW_SIZE: usize = 100;

/// Metrics collector for request statistics
#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub struct MetricsCollector {
    /// Request counts by scope
    requests_by_scope: Arc<RwLock<HashMap<String, ScopeMetrics>>>,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, Default)]
struct ScopeMetrics {
    /// Total requests (cumulative counter)
    total: u64,

    /// Accepted requests (cumulative counter)
    accepted: u64,

    /// Throttled requests (cumulative counter)
    throttled: u64,

    /// Sliding window for total request rate
    total_timestamps: VecDeque<Instant>,

    /// Sliding window for accepted request rate
    accepted_timestamps: VecDeque<Instant>,
}

#[cfg(feature = "server")]
impl MetricsCollector {
    /// Create a new metrics collector
    pub fn new() -> Self {
        MetricsCollector {
            requests_by_scope: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record a request decision
    pub async fn record_request(&self, scope: &str, throttled: bool) {
        let mut metrics = self.requests_by_scope.write().await;
        let scope_metrics = metrics.entry(scope.to_string()).or_default();
        let now = Instant::now();

        // Update counters
        scope_metrics.total += 1;
        if throttled {
            scope_metrics.throttled += 1;
        } else {
            scope_metrics.accepted += 1;
        }

        // Update sliding windows
        scope_metrics.total_timestamps.push_back(now);
        if !throttled {
            scope_metrics.accepted_timestamps.push_back(now);
        }

        // Trim windows to size limit
        if scope_metrics.total_timestamps.len() > RATE_WINDOW_SIZE {
            scope_metrics.total_timestamps.pop_front();
        }
        if scope_metrics.accepted_timestamps.len() > RATE_WINDOW_SIZE {
            scope_metrics.accepted_timestamps.pop_front();
        }
    }

    /// Export metrics in Prometheus text format
    pub async fn export_prometheus(&self) -> String {
        let metrics = self.requests_by_scope.read().await;

        let mut output = String::new();

        // Help and type declarations
        output.push_str("# HELP nenya_requests_total Total number of requests\n");
        output.push_str("# TYPE nenya_requests_total counter\n");
        output.push_str("# HELP nenya_requests_accepted Accepted requests\n");
        output.push_str("# TYPE nenya_requests_accepted counter\n");
        output.push_str("# HELP nenya_requests_throttled Throttled requests\n");
        output.push_str("# TYPE nenya_requests_throttled counter\n");

        // Metrics by scope
        for (scope, scope_metrics) in metrics.iter() {
            output.push_str(&format!(
                "nenya_requests_total{{scope=\"{}\"}} {}\n",
                scope, scope_metrics.total
            ));
            output.push_str(&format!(
                "nenya_requests_accepted{{scope=\"{}\"}} {}\n",
                scope, scope_metrics.accepted
            ));
            output.push_str(&format!(
                "nenya_requests_throttled{{scope=\"{}\"}} {}\n",
                scope, scope_metrics.throttled
            ));
        }

        output
    }

    /// Get total request count
    pub async fn total_requests(&self) -> u64 {
        let metrics = self.requests_by_scope.read().await;
        metrics.values().map(|m| m.total).sum()
    }

    /// Get accepted request count
    pub async fn accepted_requests(&self) -> u64 {
        let metrics = self.requests_by_scope.read().await;
        metrics.values().map(|m| m.accepted).sum()
    }

    /// Get throttled request count
    pub async fn throttled_requests(&self) -> u64 {
        let metrics = self.requests_by_scope.read().await;
        metrics.values().map(|m| m.throttled).sum()
    }

    /// Get total request rate (TPS) for a scope
    pub async fn total_rate(&self, scope: &str) -> f64 {
        let metrics = self.requests_by_scope.read().await;
        if let Some(scope_metrics) = metrics.get(scope) {
            Self::calculate_rate(&scope_metrics.total_timestamps)
        } else {
            0.0
        }
    }

    /// Get accepted request rate (TPS) for a scope
    pub async fn accepted_rate(&self, scope: &str) -> f64 {
        let metrics = self.requests_by_scope.read().await;
        if let Some(scope_metrics) = metrics.get(scope) {
            Self::calculate_rate(&scope_metrics.accepted_timestamps)
        } else {
            0.0
        }
    }

    /// Get throttled request rate (TPS) for a scope
    /// Calculated as total_rate - accepted_rate to avoid window skew
    pub async fn throttled_rate(&self, scope: &str) -> f64 {
        let metrics = self.requests_by_scope.read().await;
        if let Some(scope_metrics) = metrics.get(scope) {
            let total = Self::calculate_rate(&scope_metrics.total_timestamps);
            let accepted = Self::calculate_rate(&scope_metrics.accepted_timestamps);
            (total - accepted).max(0.0)
        } else {
            0.0
        }
    }

    /// Calculate rate from sliding window of timestamps
    fn calculate_rate(timestamps: &VecDeque<Instant>) -> f64 {
        if timestamps.len() < 2 {
            return 0.0;
        }

        let now = Instant::now();
        let oldest = timestamps.front().unwrap();
        let elapsed = now.duration_since(*oldest).as_secs_f64();

        if elapsed < 0.001 {
            return 0.0;
        }

        timestamps.len() as f64 / elapsed
    }
}

#[cfg(feature = "server")]
impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_record_request() {
        let collector = MetricsCollector::new();

        collector.record_request("test", false).await;
        collector.record_request("test", false).await;
        collector.record_request("test", true).await;

        assert_eq!(collector.total_requests().await, 3);
        assert_eq!(collector.accepted_requests().await, 2);
        assert_eq!(collector.throttled_requests().await, 1);
    }

    #[tokio::test]
    async fn test_multiple_scopes() {
        let collector = MetricsCollector::new();

        collector.record_request("scope1", false).await;
        collector.record_request("scope1", true).await;
        collector.record_request("scope2", false).await;
        collector.record_request("scope2", false).await;

        assert_eq!(collector.total_requests().await, 4);
        assert_eq!(collector.accepted_requests().await, 3);
        assert_eq!(collector.throttled_requests().await, 1);
    }

    #[tokio::test]
    async fn test_prometheus_export() {
        let collector = MetricsCollector::new();

        collector.record_request("test", false).await;
        collector.record_request("test", true).await;

        let output = collector.export_prometheus().await;

        assert!(output.contains("nenya_requests_total"));
        assert!(output.contains("nenya_requests_accepted"));
        assert!(output.contains("nenya_requests_throttled"));
        assert!(output.contains("scope=\"test\""));
    }
}
