//! Two-tier coordination policy: tail (local-only) vs. hot (gossiped) scopes.
//!
//! Per-user distributed throttling at large cardinality (10⁵–10⁶+ scopes)
//! cannot gossip every scope. This module holds the transport-agnostic policy
//! pieces — the compact tail-scope state, the promotion/demotion state
//! machine, and the gossip-budget eviction rule — shared by the production
//! sync loop and the deterministic simulator (like [`super::aggregate`], it
//! is compiled under both `server` and `sim` and must stay separable from
//! Chitchat specifics so a future blackboard transport can reuse it).
//!
//! The design exploits two facts:
//! 1. Load balancers spread a user's traffic roughly uniformly, so
//!    `local_rate × num_nodes` is a cheap local estimate of that user's
//!    cluster-wide rate.
//! 2. API usage is heavy-tailed: at any instant only a small fraction of
//!    users are near their limit. The tail needs no coordination — local
//!    enforcement of the equal share `limit / num_nodes` is already accurate
//!    for it.
//!
//! State machine per scope:
//! - **Tail** (default): a compact token bucket enforcing the equal share,
//!   plus a two-bucket sliding-window rate estimate for the promotion test.
//!   No gossip state, no control engine.
//! - **Hot**: a full `RateLimiter` with a control engine, published via
//!   gossip exactly like every scope was before Milestone 6. Entered when
//!   the estimated cluster utilization crosses `promote_utilization`, or
//!   when a peer starts gossiping the scope (coordination only works if all
//!   nodes carrying the scope publish their local rates). Left when the
//!   *observed* cluster rate (local + decayed peer sum) stays below
//!   `demote_utilization` for `demote_hold` (hysteresis), or when the
//!   per-node gossip budget K overflows and this scope has the lowest
//!   utilization (logged, never silent).

use std::time::{Duration, Instant};

/// Tail rate-estimator window. Aligned with the full limiter's default
/// control `update_interval` (1 s) so a promoted scope's measured rate is
/// continuous across the tier switch; not an independent tunable.
pub const TAIL_WINDOW: Duration = Duration::from_secs(1);

/// Default promotion threshold (fraction of estimated cluster utilization).
///
/// Simulator-derived (Milestone 6.4 sweep, `--ignored` test
/// `tier_threshold_sweep` in tests/simulation.rs; curve published in
/// docs/capacity-model.md): swept over Pareto workloads; the value sits at
/// the knee of the promoted-set-size vs. worst-case-overage curve.
pub const DEFAULT_PROMOTE_UTILIZATION: f64 = 0.5;

/// Default demotion threshold. From the same sweep: wide enough below
/// promotion that estimator noise at the promotion boundary cannot flap a
/// scope across both thresholds within one hold period.
pub const DEFAULT_DEMOTE_UTILIZATION: f64 = 0.25;

/// Default demotion hold (hysteresis). Must exceed the full round-trip of
/// rate information (sync interval + propagation + control interval) so a
/// scope is never demoted on a single stale/quiet observation window.
pub const DEFAULT_DEMOTE_HOLD: Duration = Duration::from_secs(10);

/// Default per-node cap on gossiped scopes. Bounds the per-link gossip
/// payload at `K × bytes_per_scope × 2/s` regardless of user count (see
/// docs/capacity-model.md for the wire math the cap is derived from).
pub const DEFAULT_GOSSIP_BUDGET: usize = 1000;

/// Per-pattern two-tier policy parameters.
#[derive(Debug, Clone, Copy)]
pub struct TierConfig {
    /// Promote a tail scope to the hot tier when
    /// `local_rate × num_nodes ≥ promote_utilization × limit`.
    pub promote_utilization: f64,

    /// Demote a hot scope when the observed cluster rate stays below
    /// `demote_utilization × limit` for `demote_hold`. Must be strictly
    /// below `promote_utilization` (hysteresis).
    pub demote_utilization: f64,

    /// How long the observed cluster rate must stay below the demotion
    /// threshold before the scope actually demotes.
    pub demote_hold: Duration,

    /// Hard per-node cap on gossiped scopes; lowest-utilization hot scopes
    /// are evicted back to the tail on overflow (and logged).
    pub gossip_budget: usize,
}

impl Default for TierConfig {
    fn default() -> Self {
        TierConfig {
            promote_utilization: DEFAULT_PROMOTE_UTILIZATION,
            demote_utilization: DEFAULT_DEMOTE_UTILIZATION,
            demote_hold: DEFAULT_DEMOTE_HOLD,
            gossip_budget: DEFAULT_GOSSIP_BUDGET,
        }
    }
}

impl TierConfig {
    /// Validate the invariants the state machine depends on.
    pub fn validate(&self) -> Result<(), String> {
        if !(self.promote_utilization > 0.0 && self.promote_utilization <= 1.0) {
            return Err(format!(
                "promote_utilization must be in (0, 1], got {}",
                self.promote_utilization
            ));
        }
        if self.demote_utilization <= 0.0 || self.demote_utilization >= self.promote_utilization {
            return Err(format!(
                "demote_utilization must be in (0, promote_utilization); got {} vs {}",
                self.demote_utilization, self.promote_utilization
            ));
        }
        if self.demote_hold.is_zero() {
            return Err("demote_hold must be positive".to_string());
        }
        if self.gossip_budget == 0 {
            return Err("gossip_budget must be at least 1".to_string());
        }
        Ok(())
    }
}

/// Compact tail-tier scope state: a token bucket enforcing the equal share
/// plus a two-bucket sliding-window rate estimate for the promotion test.
///
/// Deliberately engine-free: the whole point of the tail tier is that a
/// million idle-ish users cost tens of bytes each, not a boxed controller,
/// a peer-observation vector, and a timestamp deque (~1 KB — see
/// docs/capacity-model.md). The share to enforce is passed in per call
/// because it changes with cluster membership, which the scope itself never
/// tracks.
#[derive(Debug, Clone)]
pub struct TailScope {
    tokens: f64,
    last_refill: Instant,

    /// Two-bucket sliding-window estimator (interpolated): `prev_count`
    /// accepts landed in the last completed window, `curr_count` in the
    /// window starting at `window_start`. The estimated trailing-window
    /// rate weighs the previous bucket by its remaining overlap.
    window_start: Instant,
    prev_count: u32,
    curr_count: u32,
}

impl TailScope {
    /// Create a tail scope with a full 1-second burst at the given share
    /// (mirroring the library's adaptive `capacity = refill × 1 s` default).
    pub fn new(now: Instant, share: f64) -> Self {
        TailScope {
            tokens: share.max(0.0) * TAIL_WINDOW.as_secs_f64(),
            last_refill: now,
            window_start: now,
            prev_count: 0,
            curr_count: 0,
        }
    }

    /// Create a tail scope carrying over an explicit token balance (used
    /// when a hot scope demotes: its remaining tokens transfer, clamped by
    /// `try_admit`'s share-sized capacity on the next request).
    pub fn with_tokens(now: Instant, tokens: f64) -> Self {
        TailScope {
            tokens: tokens.max(0.0),
            last_refill: now,
            window_start: now,
            prev_count: 0,
            curr_count: 0,
        }
    }

    /// Roll the estimator windows forward to `now`.
    fn roll(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.window_start);
        if elapsed < TAIL_WINDOW {
            return;
        }
        let whole = (elapsed.as_nanos() / TAIL_WINDOW.as_nanos()) as u32;
        self.prev_count = if whole == 1 { self.curr_count } else { 0 };
        self.curr_count = 0;
        self.window_start += TAIL_WINDOW * whole;
    }

    /// Estimated local accepted rate over the trailing window
    /// (interpolated two-bucket sliding window).
    pub fn local_rate(&mut self, now: Instant) -> f64 {
        self.roll(now);
        let frac = now.duration_since(self.window_start).as_secs_f64() / TAIL_WINDOW.as_secs_f64();
        (self.prev_count as f64 * (1.0 - frac) + self.curr_count as f64) / TAIL_WINDOW.as_secs_f64()
    }

    /// Read-only variant of [`local_rate`](Self::local_rate) for stats
    /// paths that only hold a shared reference (computes the same estimate
    /// without rolling the stored windows).
    pub fn local_rate_at(&self, now: Instant) -> f64 {
        let elapsed = now.duration_since(self.window_start);
        let (prev, curr, frac) = if elapsed < TAIL_WINDOW {
            (
                self.prev_count,
                self.curr_count,
                elapsed.as_secs_f64() / TAIL_WINDOW.as_secs_f64(),
            )
        } else if elapsed < TAIL_WINDOW * 2 {
            let frac = (elapsed - TAIL_WINDOW).as_secs_f64() / TAIL_WINDOW.as_secs_f64();
            (self.curr_count, 0, frac)
        } else {
            (0, 0, 0.0)
        };
        (prev as f64 * (1.0 - frac) + curr as f64) / TAIL_WINDOW.as_secs_f64()
    }

    /// Try to admit one request at `now`, refilling at `share` tokens/sec
    /// with a 1-second burst allowance. Returns `true` if admitted.
    pub fn try_admit(&mut self, now: Instant, share: f64) -> bool {
        let share = share.max(0.0);
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let capacity = share * TAIL_WINDOW.as_secs_f64();
        self.tokens = (self.tokens + elapsed * share).min(capacity);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            self.roll(now);
            self.curr_count += 1;
            true
        } else {
            false
        }
    }

    /// Current token count (for promotion seeding).
    pub fn tokens(&self) -> f64 {
        self.tokens
    }

    /// Simulated time of the last activity (refill), for TTL eviction.
    pub fn last_activity(&self) -> Instant {
        self.last_refill
    }
}

/// Should a tail scope be promoted into gossip coordination?
///
/// `local_rate × num_nodes` estimates the scope's cluster-wide rate under
/// the uniform-routing assumption; routing skew only *over*-estimates on the
/// hot node, which promotes earlier — conservative in the right direction.
pub fn should_promote(local_rate: f64, num_nodes: usize, limit: f64, cfg: &TierConfig) -> bool {
    local_rate * num_nodes.max(1) as f64 >= cfg.promote_utilization * limit
}

/// Demotion hysteresis: tracks how long a hot scope's *observed* cluster
/// rate (local + decayed peer sum — real data, not the routing estimate)
/// has stayed below the demotion threshold.
#[derive(Debug, Clone, Default)]
pub struct DemotionTracker {
    below_since: Option<Instant>,
}

impl DemotionTracker {
    /// Feed one observation; returns `true` when the scope should demote
    /// (below `demote_utilization × limit` continuously for `demote_hold`).
    pub fn observe(
        &mut self,
        cluster_rate: f64,
        limit: f64,
        now: Instant,
        cfg: &TierConfig,
    ) -> bool {
        if cluster_rate < cfg.demote_utilization * limit {
            let since = *self.below_since.get_or_insert(now);
            now.duration_since(since) >= cfg.demote_hold
        } else {
            self.below_since = None;
            false
        }
    }
}

/// Enforce the per-node gossip budget: given every hot scope's estimated
/// utilization, return the scopes to evict back to the tail (lowest
/// utilization first, ties broken by name for determinism). Callers must
/// log what they evict — a silently truncated gossip set reads as "covered
/// everything" when it didn't.
pub fn budget_evictions(mut hot: Vec<(String, f64)>, budget: usize) -> Vec<String> {
    if hot.len() <= budget {
        return Vec::new();
    }
    // Highest utilization keeps its slot; sort descending, evict the rest
    hot.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    hot.drain(budget..).map(|(name, _)| name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TierConfig {
        TierConfig::default()
    }

    #[test]
    fn test_config_default_is_valid() {
        cfg().validate().expect("default TierConfig must validate");
    }

    #[test]
    fn test_config_rejects_inverted_thresholds() {
        let bad = TierConfig {
            promote_utilization: 0.3,
            demote_utilization: 0.5,
            ..cfg()
        };
        assert!(bad.validate().is_err());
        let equal = TierConfig {
            promote_utilization: 0.5,
            demote_utilization: 0.5,
            ..cfg()
        };
        assert!(equal.validate().is_err());
    }

    #[test]
    fn test_tail_bucket_enforces_share() {
        let start = Instant::now();
        let mut tail = TailScope::new(start, 10.0);
        // Full 1s burst at share 10 → 10 tokens
        let mut admitted = 0;
        for _ in 0..20 {
            if tail.try_admit(start, 10.0) {
                admitted += 1;
            }
        }
        assert_eq!(admitted, 10, "burst capped at share × 1s");

        // One second later the bucket has earned exactly `share` tokens
        let later = start + Duration::from_secs(1);
        let mut refilled = 0;
        for _ in 0..20 {
            if tail.try_admit(later, 10.0) {
                refilled += 1;
            }
        }
        assert_eq!(refilled, 10);
    }

    #[test]
    fn test_tail_rate_estimate_converges() {
        let start = Instant::now();
        let mut tail = TailScope::new(start, 1000.0);
        // 50 rps for 3 seconds
        for i in 0..150 {
            let t = start + Duration::from_millis(i * 20);
            assert!(tail.try_admit(t, 1000.0));
        }
        let rate = tail.local_rate(start + Duration::from_secs(3));
        assert!(
            (rate - 50.0).abs() < 5.0,
            "expected ~50 rps, got {:.1}",
            rate
        );
    }

    #[test]
    fn test_tail_rate_decays_when_idle() {
        let start = Instant::now();
        let mut tail = TailScope::new(start, 100.0);
        for i in 0..100 {
            tail.try_admit(start + Duration::from_millis(i * 10), 100.0);
        }
        // Two full idle windows later the estimate reads zero
        let rate = tail.local_rate(start + Duration::from_secs(4));
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn test_promotion_threshold() {
        let c = cfg();
        let limit = 300.0;
        // 3 nodes: promote at local ≥ 0.5 × 300 / 3 = 50
        assert!(!should_promote(49.9, 3, limit, &c));
        assert!(should_promote(50.0, 3, limit, &c));
        // Single node (no peers yet): promote at 150
        assert!(!should_promote(149.0, 1, limit, &c));
        assert!(should_promote(150.0, 1, limit, &c));
        // num_nodes 0 treated as 1 (defensive)
        assert!(should_promote(150.0, 0, limit, &c));
    }

    #[test]
    fn test_demotion_requires_sustained_low_utilization() {
        let c = cfg();
        let start = Instant::now();
        let mut tracker = DemotionTracker::default();
        let limit = 300.0;
        let low = c.demote_utilization * limit - 1.0;

        assert!(!tracker.observe(low, limit, start, &c), "no instant demote");
        assert!(
            !tracker.observe(low, limit, start + c.demote_hold / 2, &c),
            "hold not yet elapsed"
        );
        assert!(
            tracker.observe(low, limit, start + c.demote_hold, &c),
            "hold elapsed → demote"
        );
    }

    #[test]
    fn test_demotion_hold_resets_on_activity() {
        let c = cfg();
        let start = Instant::now();
        let mut tracker = DemotionTracker::default();
        let limit = 300.0;
        let low = c.demote_utilization * limit - 1.0;
        let high = c.demote_utilization * limit + 1.0;

        assert!(!tracker.observe(low, limit, start, &c));
        // Activity above the threshold resets the hold
        assert!(!tracker.observe(high, limit, start + c.demote_hold / 2, &c));
        assert!(
            !tracker.observe(
                low,
                limit,
                start + c.demote_hold + Duration::from_secs(1),
                &c
            ),
            "hold restarted after reset"
        );
    }

    #[test]
    fn test_budget_evicts_lowest_utilization() {
        let hot = vec![
            ("a".to_string(), 0.9),
            ("b".to_string(), 0.1),
            ("c".to_string(), 0.5),
            ("d".to_string(), 0.3),
        ];
        let evicted = budget_evictions(hot, 2);
        assert_eq!(evicted, vec!["d".to_string(), "b".to_string()]);
    }

    #[test]
    fn test_budget_no_eviction_under_cap() {
        let hot = vec![("a".to_string(), 0.9), ("b".to_string(), 0.1)];
        assert!(budget_evictions(hot, 2).is_empty());
    }

    #[test]
    fn test_budget_deterministic_tie_break() {
        let hot = vec![
            ("b".to_string(), 0.5),
            ("a".to_string(), 0.5),
            ("c".to_string(), 0.5),
        ];
        // Ties broken by name ascending in the kept ordering → "c" evicted
        assert_eq!(budget_evictions(hot, 2), vec!["c".to_string()]);
    }
}
