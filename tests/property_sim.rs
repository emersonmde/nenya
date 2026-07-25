//! Property-based tests for the aggregation/decay logic and token bucket
//! (Milestone 4.5).
//!
//! These verify library invariants over arbitrary inputs, complementing the
//! scenario simulations (which check dynamics at specific operating points).

#![cfg(feature = "sim")]

use std::time::{Duration, Instant};

use nenya::gossip::aggregate::{aggregate_peer_rates, staleness_weight, PeerObservation};
use nenya::pid_controller::PIDControllerBuilder;
use nenya::RateLimiterBuilder;
use proptest::prelude::*;

proptest! {
    /// Decay weight is always within [0, 1], for any configuration —
    /// including degenerate ones (stale_timeout inside the full-weight
    /// window)
    #[test]
    fn staleness_weight_in_unit_interval(
        age_ms in 0u64..60_000,
        sync_ms in 1u64..5_000,
        timeout_ms in 1u64..60_000,
    ) {
        let w = staleness_weight(
            Duration::from_millis(age_ms),
            Duration::from_millis(sync_ms),
            Duration::from_millis(timeout_ms),
        );
        prop_assert!((0.0..=1.0).contains(&w), "weight {} out of [0,1]", w);
    }

    /// Decay weight is monotonically non-increasing in age
    #[test]
    fn staleness_weight_monotonic_in_age(
        sync_ms in 1u64..5_000,
        timeout_ms in 1u64..60_000,
        ages_ms in proptest::collection::vec(0u64..60_000, 2..20),
    ) {
        let sync = Duration::from_millis(sync_ms);
        let timeout = Duration::from_millis(timeout_ms);
        let mut sorted = ages_ms;
        sorted.sort_unstable();
        let mut prev = f64::INFINITY;
        for age in sorted {
            let w = staleness_weight(Duration::from_millis(age), sync, timeout);
            prop_assert!(w <= prev + 1e-12, "weight increased with age");
            prev = w;
        }
    }

    /// A peer at or past stale_timeout contributes exactly zero and does not
    /// count as live — no phantom load, for any rate value
    #[test]
    fn stale_peer_contributes_nothing(
        rate in 0.0f64..1e9,
        age_past_timeout_ms in 0u64..600_000,
    ) {
        let timeout = Duration::from_secs(10);
        let peer = PeerObservation {
            node_id: "p".to_string(),
            age: timeout + Duration::from_millis(age_past_timeout_ms),
            scope_rates: [("s".to_string(), rate)].into_iter().collect(),
        };
        let agg = aggregate_peer_rates(&[peer], Duration::from_millis(500), timeout);
        prop_assert_eq!(agg.live_peers, 0);
        prop_assert!(agg.scope_rates.get("s").copied().unwrap_or(0.0) == 0.0);
    }

    /// Aggregated external rate is non-negative and never exceeds the sum of
    /// live peers' published rates (decay weights are ≤ 1)
    #[test]
    fn aggregate_bounded_by_live_sum(
        peers in proptest::collection::vec((0u64..30_000, 0.0f64..10_000.0), 0..8),
    ) {
        let sync = Duration::from_millis(500);
        let timeout = Duration::from_secs(10);
        let observations: Vec<PeerObservation> = peers
            .iter()
            .enumerate()
            .map(|(i, (age_ms, rate))| PeerObservation {
                node_id: format!("p{}", i),
                age: Duration::from_millis(*age_ms),
                scope_rates: [("s".to_string(), *rate)].into_iter().collect(),
            })
            .collect();

        let agg = aggregate_peer_rates(&observations, sync, timeout);
        let external = agg.scope_rates.get("s").copied().unwrap_or(0.0);

        let live_sum: f64 = observations
            .iter()
            .filter(|o| o.age < timeout)
            .map(|o| o.scope_rates["s"])
            .sum();

        prop_assert!(external >= 0.0);
        prop_assert!(
            external <= live_sum + 1e-9,
            "external {} exceeds live sum {}",
            external,
            live_sum
        );
        prop_assert!(agg.live_peers <= observations.len());
    }

    /// Token count never exceeds bucket capacity, over arbitrary request
    /// timings and PID-adjusted refill rates
    #[test]
    fn token_bucket_never_exceeds_capacity(
        capacity in 1.0f64..1_000.0,
        target in 1.0f64..1_000.0,
        offsets_ms in proptest::collection::vec(0u64..30_000, 1..100),
    ) {
        let base = Instant::now();
        let pid = PIDControllerBuilder::new(target).kp(0.5).ki(0.02).kd(0.08).build();
        let mut limiter = RateLimiterBuilder::new(target)
            .min_rate(target * 0.5)
            .max_rate(target * 2.0)
            .bucket_capacity(capacity)
            .pid_controller(pid)
            .initial_timestamp(base)
            .build();

        prop_assert!(limiter.tokens() <= capacity);

        let mut sorted = offsets_ms;
        sorted.sort_unstable();
        for offset in sorted {
            limiter.should_throttle_at(base + Duration::from_millis(offset));
            prop_assert!(
                limiter.tokens() <= capacity,
                "tokens {} exceed capacity {}",
                limiter.tokens(),
                capacity
            );
        }
    }
}
