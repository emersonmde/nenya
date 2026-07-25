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
/// `tier_threshold_sweep` in tests/simulation.rs, seed 42; tables published
/// in docs/capacity-model.md). Key finding: the expected promoted-set-size
/// vs. worst-case-overage knee does not exist — per-user overage is
/// structurally absent at every threshold in {0.3..0.8} (an unpromoted
/// scope is capped at `limit / n` per node, so its cluster total cannot
/// exceed the limit; skew triggers promotion on the hot node), and the
/// ramp transient is flat (~1.6 × limit worst 1 s window) beyond 0.3. The
/// threshold therefore only trades promoted-set size (35 → 7 scopes per
/// 100k Zipf users across the sweep) against coordination headroom — how
/// far below the limit a ramping user is already under engine control.
/// 0.5 keeps 2× headroom at 17 promoted per 100k users; raising it toward
/// 0.8 halves the hot set with no measured downside in these scenarios.
pub const DEFAULT_PROMOTE_UTILIZATION: f64 = 0.5;

/// Default demotion threshold. From the same sweep (flap axis): the
/// highest value with zero measured flapping — a user parked exactly at
/// the demotion boundary for 300 s produces 3 promotion events (exactly
/// one per node) at 0.25, versus 12 at 0.35 and 23 at 0.45. Higher is
/// better for hot-set shedding, so 0.25 is the knee.
pub const DEFAULT_DEMOTE_UTILIZATION: f64 = 0.25;

/// Default demotion hold (hysteresis). Must exceed the full round-trip of
/// rate information (sync interval + propagation + control interval) so a
/// scope is never demoted on a single stale/quiet observation window.
pub const DEFAULT_DEMOTE_HOLD: Duration = Duration::from_secs(10);

/// Default TTL for idle scopes (no accepted request within this window →
/// evicted at the next sweep; sweeps run every TTL/2).
///
/// Evicting an idle *tail* scope is behaviorally lossless once it has been
/// idle longer than `2 × TAIL_WINDOW`: the bucket refills to its full
/// share burst within one window anyway and the rate estimate has decayed
/// to zero, so recreation on the next request reproduces the exact state.
/// The knob therefore only trades recreation/allocation churn for
/// periodic users against idle-set memory (`memory ≈ users active within
/// TTL × bytes/scope`); 60 s keeps minute-scale periodic users allocated
/// while bounding the idle set at roughly a minute of unique traffic. Hot
/// scopes are never TTL-evicted directly — an idle hot scope demotes
/// through the hysteresis first and then ages out as a tail scope.
pub const DEFAULT_SCOPE_TTL: Duration = Duration::from_secs(60);

/// Default promotion-estimator window (see `TierConfig::estimator_window`).
///
/// Simulator-derived (Milestone 6.4 sweep, seed 42): at sparse rates the
/// promotion threshold is only a handful of requests per window, so short
/// windows read Poisson clumps as sustained rates — a 1 s window promotes
/// 78 scopes per 100k Zipf users (~10 truly over threshold) and promotes a
/// 2 rps user 29 s before it ever ramps. 8 s is the first window with no
/// spurious promotion of the sub-threshold phase (17 promoted, 1.0 s
/// promotion lag); 12/16 s shave only 2–4 more scopes while adding 1.5–2 s
/// of lag.
pub const DEFAULT_TAIL_ESTIMATOR_WINDOW: Duration = Duration::from_secs(8);

/// Default tail burst depth as a fraction of the limit (per-node bucket
/// capacity = `max(share, tail_burst_fraction × limit) × 1 s`, floored at
/// one token — the floor removes the sub-token starvation edge on
/// clusters larger than the per-user rps limit).
///
/// Simulator-derived (`tier_threshold_sweep` axis 4, seed 42; tables in
/// docs/capacity-model.md). Depth doesn't change long-run admission
/// (refill stays at the fair share; deeper buckets refill proportionally
/// slower) or steady per-user overage (flat at every fraction) — it
/// trades how much of a *concentrated* burst one node absorbs
/// (`fraction × limit`) against the worst-case cold-bucket spike for a
/// synchronized, perfectly spread burst (`n × fraction × limit` per
/// scope, at most once per bucket-refill period). 0.5 admits half a
/// page-load-style burst through one node, keeps the service-pattern
/// join burst inside the Milestone 5 budget (autoscale 2698 vs the 4000
/// ceiling; 1.0 measures 2793), and matches the promotion threshold —
/// one node absorbs bursts up to the utilization level at which
/// coordination takes over, and a burst big enough to trip promotion
/// carries its remaining tail tokens into the promoted limiter instead
/// of being truncated. Per-user-focused deployments can raise it to 1.0
/// for full Redis-style per-user burst semantics; DDoS-sensitive ones
/// can lower it toward 0.
pub const DEFAULT_TAIL_BURST_FRACTION: f64 = 0.5;

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

    /// Promotion-estimator window: the tail-scope rate estimate feeding
    /// the promotion test spans this long. Longer = slower promotion
    /// detection but less Poisson noise at sparse rates (a 1 s window
    /// reads two clumped arrivals as 2 rps and spuriously promotes users
    /// far below their limit). Sweep-derived default.
    pub estimator_window: Duration,

    /// Tail burst depth as a fraction of the limit: per-node bucket
    /// capacity is `max(share, tail_burst_fraction × limit) × 1 s`,
    /// floored at one token. Refill stays at the fair share, so this
    /// trades *concentrated*-burst tolerance (a client bursting through
    /// one node) against the worst-case cold-bucket spike
    /// (`n × capacity` for a perfectly spread synchronized burst); the
    /// long-run admission volume is depth-independent. Sweep-derived
    /// default.
    pub tail_burst_fraction: f64,
}

impl Default for TierConfig {
    fn default() -> Self {
        TierConfig {
            promote_utilization: DEFAULT_PROMOTE_UTILIZATION,
            demote_utilization: DEFAULT_DEMOTE_UTILIZATION,
            demote_hold: DEFAULT_DEMOTE_HOLD,
            gossip_budget: DEFAULT_GOSSIP_BUDGET,
            estimator_window: DEFAULT_TAIL_ESTIMATOR_WINDOW,
            tail_burst_fraction: DEFAULT_TAIL_BURST_FRACTION,
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
        if self.estimator_window.is_zero() {
            return Err("estimator_window must be positive".to_string());
        }
        if !(0.0..=1.0).contains(&self.tail_burst_fraction) {
            return Err(format!(
                "tail_burst_fraction must be in [0, 1], got {}",
                self.tail_burst_fraction
            ));
        }
        Ok(())
    }
}

/// Interpolated two-bucket sliding-window rate estimator: `prev_count`
/// events landed in the last completed window, `curr_count` in the window
/// starting at `window_start`. The estimated trailing-window rate weighs
/// the previous bucket by its remaining overlap. ~32 bytes — used per tail
/// scope (promotion test) and per pattern (the gossiped tail aggregate).
#[derive(Debug, Clone)]
pub struct RateWindow {
    window_start: Instant,
    window_len: Duration,
    prev_count: u32,
    curr_count: u32,
}

impl RateWindow {
    /// Estimator with the default [`TAIL_WINDOW`] length.
    pub fn new(now: Instant) -> Self {
        Self::with_len(now, TAIL_WINDOW)
    }

    /// Estimator over an explicit window length. Longer windows trade
    /// promotion-detection lag for lower Poisson noise at sparse rates —
    /// see `TierConfig::estimator_window`.
    pub fn with_len(now: Instant, window_len: Duration) -> Self {
        RateWindow {
            window_start: now,
            window_len,
            prev_count: 0,
            curr_count: 0,
        }
    }

    /// Roll the windows forward to `now`.
    fn roll(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.window_start);
        if elapsed < self.window_len {
            return;
        }
        let whole = (elapsed.as_nanos() / self.window_len.as_nanos()) as u32;
        self.prev_count = if whole == 1 { self.curr_count } else { 0 };
        self.curr_count = 0;
        self.window_start += self.window_len * whole;
    }

    /// Record one event at `now`.
    pub fn record(&mut self, now: Instant) {
        self.roll(now);
        self.curr_count += 1;
    }

    /// Estimated trailing-window rate (events/sec), rolling forward first.
    pub fn rate(&mut self, now: Instant) -> f64 {
        self.roll(now);
        let frac =
            now.duration_since(self.window_start).as_secs_f64() / self.window_len.as_secs_f64();
        (self.prev_count as f64 * (1.0 - frac) + self.curr_count as f64)
            / self.window_len.as_secs_f64()
    }

    /// Read-only variant of [`rate`](Self::rate) for stats paths that only
    /// hold a shared reference (same estimate, no roll).
    pub fn rate_at(&self, now: Instant) -> f64 {
        let elapsed = now.duration_since(self.window_start);
        let (prev, curr, frac) = if elapsed < self.window_len {
            (
                self.prev_count,
                self.curr_count,
                elapsed.as_secs_f64() / self.window_len.as_secs_f64(),
            )
        } else if elapsed < self.window_len * 2 {
            let frac = (elapsed - self.window_len).as_secs_f64() / self.window_len.as_secs_f64();
            (self.curr_count, 0, frac)
        } else {
            (0, 0, 0.0)
        };
        (prev as f64 * (1.0 - frac) + curr as f64) / self.window_len.as_secs_f64()
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
    window: RateWindow,
}

impl TailScope {
    /// Create a tail scope with a full bucket at the given capacity (see
    /// [`tail_capacity`]). `estimator_window` is the promotion-estimator
    /// length (`TierConfig::estimator_window`).
    pub fn new(now: Instant, capacity: f64, estimator_window: Duration) -> Self {
        TailScope {
            tokens: capacity.max(0.0),
            last_refill: now,
            window: RateWindow::with_len(now, estimator_window),
        }
    }

    /// Create a tail scope carrying over an explicit token balance (used
    /// when a hot scope demotes: its remaining tokens transfer, clamped by
    /// `try_admit`'s share-sized capacity on the next request).
    pub fn with_tokens(now: Instant, tokens: f64, estimator_window: Duration) -> Self {
        TailScope {
            tokens: tokens.max(0.0),
            last_refill: now,
            window: RateWindow::with_len(now, estimator_window),
        }
    }

    /// Estimated local accepted rate over the trailing window.
    pub fn local_rate(&mut self, now: Instant) -> f64 {
        self.window.rate(now)
    }

    /// Read-only variant of [`local_rate`](Self::local_rate) for stats
    /// paths that only hold a shared reference.
    pub fn local_rate_at(&self, now: Instant) -> f64 {
        self.window.rate_at(now)
    }

    /// Try to admit one request at `now`, refilling at `share` tokens/sec
    /// up to `capacity` tokens (see [`tail_capacity`]). Returns `true` if
    /// admitted.
    pub fn try_admit(&mut self, now: Instant, share: f64, capacity: f64) -> bool {
        let share = share.max(0.0);
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * share).min(capacity.max(0.0));
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            self.window.record(now);
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

/// Per-node tail bucket capacity in tokens: one second of the greater of
/// the fair share and the configured burst fraction of the limit, floored
/// at one token so a bucket can always eventually admit (see
/// [`DEFAULT_TAIL_BURST_FRACTION`] for the starvation edge this floor
/// removes).
pub fn tail_capacity(share: f64, limit: f64, cfg: &TierConfig) -> f64 {
    (share.max(cfg.tail_burst_fraction * limit) * TAIL_WINDOW.as_secs_f64()).max(1.0)
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
        let mut tail = TailScope::new(start, 10.0, TAIL_WINDOW);
        // Full bucket at capacity 10 → 10 tokens
        let mut admitted = 0;
        for _ in 0..20 {
            if tail.try_admit(start, 10.0, 10.0) {
                admitted += 1;
            }
        }
        assert_eq!(admitted, 10, "burst capped at capacity");

        // One second later the bucket has earned exactly `share` tokens
        let later = start + Duration::from_secs(1);
        let mut refilled = 0;
        for _ in 0..20 {
            if tail.try_admit(later, 10.0, 10.0) {
                refilled += 1;
            }
        }
        assert_eq!(refilled, 10);
    }

    #[test]
    fn test_tail_rate_estimate_converges() {
        let start = Instant::now();
        let mut tail = TailScope::new(start, 1000.0, TAIL_WINDOW);
        // 50 rps for 3 seconds
        for i in 0..150 {
            let t = start + Duration::from_millis(i * 20);
            assert!(tail.try_admit(t, 1000.0, 1000.0));
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
        let mut tail = TailScope::new(start, 100.0, TAIL_WINDOW);
        for i in 0..100 {
            tail.try_admit(start + Duration::from_millis(i * 10), 100.0, 100.0);
        }
        // Two full idle windows later the estimate reads zero
        let rate = tail.local_rate(start + Duration::from_secs(4));
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn test_tail_capacity_floor_prevents_starvation() {
        // Sub-token shares (cluster larger than the per-user limit) must
        // still be admissible: capacity floors at one token
        let cfg = TierConfig {
            tail_burst_fraction: 0.0,
            ..TierConfig::default()
        };
        let share = 0.2; // limit 10, 50 nodes
        let capacity = tail_capacity(share, 10.0, &cfg);
        assert!(capacity >= 1.0, "capacity {} below one token", capacity);

        let start = Instant::now();
        let mut tail = TailScope::new(start, capacity, TAIL_WINDOW);
        assert!(
            tail.try_admit(start, share, capacity),
            "a fresh sub-token-share bucket must admit its first request"
        );
        // And keeps admitting at the share rate: one token every 5s
        assert!(!tail.try_admit(start + Duration::from_secs(1), share, capacity));
        assert!(tail.try_admit(start + Duration::from_secs(6), share, capacity));
    }

    #[test]
    fn test_tail_capacity_burst_fraction() {
        let cfg = TierConfig::default(); // tail_burst_fraction 0.5
                                         // Large cluster: fraction of the limit dominates the share
        assert_eq!(tail_capacity(0.4, 10.0, &cfg), 5.0);
        // Small cluster: the share dominates
        assert_eq!(tail_capacity(100.0, 300.0, &cfg), 150.0);
        assert_eq!(tail_capacity(200.0, 300.0, &cfg), 200.0);
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
