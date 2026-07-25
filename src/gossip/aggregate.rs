//! Age-weighted aggregation of peer rate observations
//!
//! This module is transport-agnostic: it consumes `(rate, age)` observations
//! without caring whether they arrived via Chitchat gossip or any other
//! transport, so the same decay logic can be reused by the deterministic
//! simulator (Milestone 4) and a future blackboard transport.
//!
//! Ages are measured locally (time since the observation was last seen to
//! change, on the local monotonic clock) — peer wall-clock timestamps are never
//! compared across nodes, so clock skew cannot affect decay.

use std::collections::HashMap;
use std::time::Duration;

/// A single peer's most recent per-scope rates, with the locally measured age
/// of that observation.
#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub struct PeerObservation {
    /// Peer node identifier
    pub node_id: String,

    /// Time since this peer's state was last observed to change, measured on
    /// the local monotonic clock (age-at-receipt)
    pub age: Duration,

    /// Per-scope accepted rates reported by this peer
    pub scope_rates: HashMap<String, f64>,
}

/// Result of aggregating peer observations
#[cfg(feature = "server")]
#[derive(Debug, Clone, Default)]
pub struct AggregatedRates {
    /// Sum of age-weighted peer rates per scope
    pub scope_rates: HashMap<String, f64>,

    /// Number of peers considered live (staleness weight > 0)
    pub live_peers: usize,
}

/// Compute the staleness weight for an observation of the given age.
///
/// - Full weight (1.0) for ages up to `2 × sync_interval` — a healthy peer
///   publishes every `sync_interval`, so anything fresher than two intervals
///   is just normal propagation delay, not staleness
/// - Linear decay from 1.0 to 0.0 between `2 × sync_interval` and
///   `stale_timeout`
/// - Zero at and beyond `stale_timeout` — the peer is presumed dead or
///   partitioned and its last known rate is phantom load
///
/// Degenerate configurations (`stale_timeout <= 2 × sync_interval`) collapse
/// to a hard cutoff at `stale_timeout`.
#[cfg(feature = "server")]
pub fn staleness_weight(age: Duration, sync_interval: Duration, stale_timeout: Duration) -> f64 {
    if age >= stale_timeout {
        return 0.0;
    }

    let full_weight_window = 2 * sync_interval;
    if age <= full_weight_window {
        return 1.0;
    }

    let decay_span = stale_timeout.saturating_sub(full_weight_window);
    if decay_span.is_zero() {
        // Degenerate config: no decay span, hard cutoff at stale_timeout
        return 1.0;
    }

    let into_decay = age - full_weight_window;
    1.0 - into_decay.as_secs_f64() / decay_span.as_secs_f64()
}

/// Aggregate peer observations into per-scope external rates, weighting each
/// peer's contribution by freshness and dropping peers past `stale_timeout`.
#[cfg(feature = "server")]
pub fn aggregate_peer_rates(
    peers: &[PeerObservation],
    sync_interval: Duration,
    stale_timeout: Duration,
) -> AggregatedRates {
    let mut result = AggregatedRates::default();

    for peer in peers {
        let weight = staleness_weight(peer.age, sync_interval, stale_timeout);
        if weight <= 0.0 {
            continue;
        }

        result.live_peers += 1;
        for (scope, rate) in &peer.scope_rates {
            *result.scope_rates.entry(scope.clone()).or_default() += rate * weight;
        }
    }

    result
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    const SYNC: Duration = Duration::from_millis(500);
    const TIMEOUT: Duration = Duration::from_secs(10);

    fn peer(node_id: &str, age: Duration, rates: &[(&str, f64)]) -> PeerObservation {
        PeerObservation {
            node_id: node_id.to_string(),
            age,
            scope_rates: rates.iter().map(|(s, r)| (s.to_string(), *r)).collect(),
        }
    }

    #[test]
    fn test_full_weight_within_window() {
        assert_eq!(staleness_weight(Duration::ZERO, SYNC, TIMEOUT), 1.0);
        assert_eq!(
            staleness_weight(Duration::from_millis(500), SYNC, TIMEOUT),
            1.0
        );
        assert_eq!(
            staleness_weight(Duration::from_millis(1000), SYNC, TIMEOUT),
            1.0
        );
    }

    #[test]
    fn test_zero_weight_at_and_past_stale_timeout() {
        assert_eq!(staleness_weight(TIMEOUT, SYNC, TIMEOUT), 0.0);
        assert_eq!(
            staleness_weight(Duration::from_secs(60), SYNC, TIMEOUT),
            0.0
        );
    }

    #[test]
    fn test_decay_is_monotonic_nonincreasing() {
        let mut prev = f64::INFINITY;
        for ms in (0..=11_000).step_by(100) {
            let w = staleness_weight(Duration::from_millis(ms), SYNC, TIMEOUT);
            assert!(
                w <= prev,
                "weight increased at age {}ms: {} > {}",
                ms,
                w,
                prev
            );
            assert!((0.0..=1.0).contains(&w));
            prev = w;
        }
    }

    #[test]
    fn test_decay_midpoint() {
        // Decay spans 1s..10s; midpoint 5.5s should weigh 0.5
        let w = staleness_weight(Duration::from_millis(5500), SYNC, TIMEOUT);
        assert!((w - 0.5).abs() < 1e-9, "expected 0.5, got {}", w);
    }

    #[test]
    fn test_degenerate_config_hard_cutoff() {
        // stale_timeout inside the full-weight window: hard cutoff, no panic
        let timeout = Duration::from_millis(800);
        assert_eq!(
            staleness_weight(Duration::from_millis(700), SYNC, timeout),
            1.0
        );
        assert_eq!(
            staleness_weight(Duration::from_millis(800), SYNC, timeout),
            0.0
        );
    }

    #[test]
    fn test_aggregate_fresh_peers_full_weight() {
        let peers = vec![
            peer("a", Duration::ZERO, &[("s1", 10.0), ("s2", 5.0)]),
            peer("b", Duration::from_millis(500), &[("s1", 20.0)]),
        ];
        let agg = aggregate_peer_rates(&peers, SYNC, TIMEOUT);
        assert_eq!(agg.live_peers, 2);
        assert_eq!(agg.scope_rates["s1"], 30.0);
        assert_eq!(agg.scope_rates["s2"], 5.0);
    }

    #[test]
    fn test_stale_peer_contributes_zero() {
        let peers = vec![
            peer("live", Duration::ZERO, &[("s1", 10.0)]),
            peer("dead", TIMEOUT, &[("s1", 1000.0)]),
        ];
        let agg = aggregate_peer_rates(&peers, SYNC, TIMEOUT);
        assert_eq!(agg.live_peers, 1);
        assert_eq!(agg.scope_rates["s1"], 10.0);
    }

    #[test]
    fn test_decaying_peer_contributes_partial_rate() {
        let peers = vec![peer("aging", Duration::from_millis(5500), &[("s1", 100.0)])];
        let agg = aggregate_peer_rates(&peers, SYNC, TIMEOUT);
        assert_eq!(agg.live_peers, 1);
        assert!((agg.scope_rates["s1"] - 50.0).abs() < 1e-6);
    }

    #[test]
    fn test_aggregate_empty() {
        let agg = aggregate_peer_rates(&[], SYNC, TIMEOUT);
        assert_eq!(agg.live_peers, 0);
        assert!(agg.scope_rates.is_empty());
    }
}
