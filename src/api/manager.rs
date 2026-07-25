//! Multi-scope rate limit manager
//!
//! Manages multiple rate limiters with pattern-based configuration and auto-creation.

use crate::engine::{BayesianEngine, BayesianParams, EngineKind, HybridEngine, PidEngine};
use crate::gossip::aggregate::{AggregatedRates, PeerObservation};
use crate::gossip::tier::{
    budget_evictions, should_promote, DemotionTracker, RateWindow, TailScope, TierConfig,
};
use crate::pid_controller::PIDControllerBuilder;
use crate::{RateLimiter, RateLimiterBuilder};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[cfg(feature = "server")]
use serde::{Deserialize, Serialize};

/// Pattern configuration for scope matching
#[cfg(feature = "server")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopePattern {
    /// Pattern to match (exact match or wildcard with `*`)
    pub pattern: String,

    /// Target rate for this pattern
    pub target_rate: f64,

    /// Minimum rate bound (defaults to 0.5 * target_rate)
    pub min_rate: Option<f64>,

    /// Maximum rate bound (defaults to 2.0 * target_rate)
    pub max_rate: Option<f64>,

    /// PID proportional gain
    pub kp: Option<f64>,

    /// PID integral gain
    pub ki: Option<f64>,

    /// PID derivative gain
    pub kd: Option<f64>,

    /// Integral anti-windup clamp as a fraction of target_rate
    /// (defaults to 0.2)
    pub error_limit_frac: Option<f64>,

    /// Whether this is a distributed rate limit (cluster-wide target)
    pub distributed: bool,

    /// Control engine for scopes matching this pattern. Always explicit
    /// config (`pid` | `bayesian` | `hybrid`); never selected at runtime.
    #[serde(default)]
    pub engine: EngineKind,

    /// Estimator process noise `q` (rps²/s) for bayesian/hybrid
    pub process_noise: Option<f64>,

    /// Estimator measurement noise `r` (rps²) for bayesian/hybrid
    pub measurement_noise: Option<f64>,

    /// Admission confidence multiplier `z` for the bayesian engine
    pub confidence_z: Option<f64>,

    /// Two-tier promotion threshold: a scope enters gossip coordination
    /// when `local_rate × num_nodes ≥ promote_utilization × target_rate`.
    /// Only meaningful for distributed patterns. Defaults to the
    /// sweep-derived `tier::DEFAULT_PROMOTE_UTILIZATION`.
    #[serde(default)]
    pub promote_utilization: Option<f64>,

    /// Two-tier demotion threshold (fraction of target; must be below
    /// `promote_utilization`). Defaults to `tier::DEFAULT_DEMOTE_UTILIZATION`.
    #[serde(default)]
    pub demote_utilization: Option<f64>,

    /// Demotion hysteresis hold in seconds. Defaults to
    /// `tier::DEFAULT_DEMOTE_HOLD`.
    #[serde(default)]
    pub demote_hold_secs: Option<f64>,
}

#[cfg(feature = "server")]
impl ScopePattern {
    /// Create a default pattern
    pub fn default_pattern(target_rate: f64) -> Self {
        ScopePattern {
            pattern: "*".to_string(),
            target_rate,
            min_rate: None,
            max_rate: None,
            kp: None,
            ki: None,
            kd: None,
            error_limit_frac: None,
            distributed: false,
            engine: EngineKind::Pid,
            process_noise: None,
            measurement_noise: None,
            confidence_z: None,
            promote_utilization: None,
            demote_utilization: None,
            demote_hold_secs: None,
        }
    }

    /// Two-tier policy for scopes matching this pattern; unset fields fall
    /// back to the sweep-derived defaults in `gossip::tier`. The gossip
    /// budget is node-level (it caps the node's total gossip payload, not a
    /// pattern's) and is passed in by the manager.
    pub fn tier_config(&self, gossip_budget: usize) -> TierConfig {
        let defaults = TierConfig::default();
        TierConfig {
            promote_utilization: self
                .promote_utilization
                .unwrap_or(defaults.promote_utilization),
            demote_utilization: self
                .demote_utilization
                .unwrap_or(defaults.demote_utilization),
            demote_hold: self
                .demote_hold_secs
                .map(Duration::from_secs_f64)
                .unwrap_or(defaults.demote_hold),
            gossip_budget,
        }
    }

    /// Estimator parameters for the bayesian/hybrid engines, with the
    /// membership horizon aligned to the transport's stale timeout.
    /// Unset fields fall back to the engine-appropriate simulator-derived
    /// default (the hybrid engine wants a fast filter, the pure Bayesian a
    /// slow one — see `BayesianParams`).
    fn estimator_params(&self, stale_timeout: Duration) -> BayesianParams {
        let defaults = match self.engine {
            EngineKind::Hybrid => BayesianParams::hybrid_default(),
            _ => BayesianParams::default(),
        };
        BayesianParams {
            process_noise: self.process_noise.unwrap_or(defaults.process_noise),
            measurement_noise: self.measurement_noise.unwrap_or(defaults.measurement_noise),
            confidence_z: self.confidence_z.unwrap_or(defaults.confidence_z),
            stale_timeout,
        }
    }

    /// Check if this pattern matches a scope name
    fn matches(&self, scope: &str) -> PatternMatch {
        if self.pattern == scope {
            PatternMatch::Exact
        } else if self.pattern.ends_with('*') {
            let prefix = &self.pattern[..self.pattern.len() - 1];
            if scope.starts_with(prefix) {
                PatternMatch::Wildcard
            } else {
                PatternMatch::None
            }
        } else {
            PatternMatch::None
        }
    }

    /// Get min rate (defaults to 0.5 * target_rate)
    pub fn get_min_rate(&self) -> f64 {
        self.min_rate.unwrap_or(self.target_rate * 0.5)
    }

    /// Get max rate (defaults to 2.0 * target_rate)
    pub fn get_max_rate(&self) -> f64 {
        self.max_rate.unwrap_or(self.target_rate * 2.0)
    }

    /// Get PID Kp (defaults to 0.8)
    pub fn get_kp(&self) -> f64 {
        self.kp.unwrap_or(0.8)
    }

    /// Get PID Ki (defaults to 0.05)
    pub fn get_ki(&self) -> f64 {
        self.ki.unwrap_or(0.05)
    }

    /// Get PID Kd (defaults to 0.04)
    pub fn get_kd(&self) -> f64 {
        self.kd.unwrap_or(0.04)
    }

    /// Get the integral anti-windup clamp (defaults to 0.2 × target_rate).
    ///
    /// Without a clamp, any sustained gap between setpoint and achievable
    /// rate (e.g. a partitioned minority whose fair share exceeds its
    /// offered load) winds the integral term up without bound, and the node
    /// overshoots its share for tens of seconds after conditions change.
    /// The 0.2 fraction is simulator-derived: in the Milestone 4 scenario
    /// matrix, 0.1/0.2/0.5 all bound windup with marginal trade-offs
    /// (smaller = less overshoot, larger = less undershoot); 0.2 is the
    /// midpoint and cut post-heal re-convergence in the partition scenario
    /// from ~60s to ~5s versus no clamp.
    pub fn get_error_limit(&self) -> f64 {
        self.error_limit_frac.unwrap_or(0.2) * self.target_rate
    }
}

/// Pattern match type
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PatternMatch {
    None,
    Wildcard,
    Exact,
}

/// Throttle decision result
#[cfg(feature = "server")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThrottleDecision {
    /// Whether the request should be throttled
    pub should_throttle: bool,

    /// Current local accepted rate for this scope
    pub local_accepted_rate: f64,

    /// Current refill rate (tokens per second)
    pub refill_rate: f64,

    /// Number of peers in cluster (0 if not distributed)
    pub num_peers: usize,
}

/// Scope statistics
#[cfg(feature = "server")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeStats {
    /// Scope name
    pub name: String,

    /// Target rate
    pub target_rate: f64,

    /// Current local accepted rate
    pub local_accepted_rate: f64,

    /// Number of peers (0 if not distributed)
    pub num_peers: usize,

    /// Whether distributed mode is enabled
    pub distributed: bool,

    /// Coordination tier: `local` (non-distributed), `tail` (local equal
    /// share, not gossiped), or `hot` (full gossip coordination)
    pub tier: String,
}

/// Read-only view of one scope's state (any tier), for stats endpoints
#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub struct ScopeSnapshot {
    pub target_rate: f64,
    pub local_accepted_rate: f64,
    pub refill_rate: f64,
    pub num_peers: usize,
    pub external_rate: f64,
    pub tier: &'static str,
}

/// One scope's coordination state.
///
/// Non-distributed patterns get a full limiter that never gossips
/// (`Local`). Distributed patterns start in the compact `Tail` tier and are
/// promoted to `Hot` (full limiter + gossip) by the two-tier policy in
/// `gossip::tier`.
#[cfg(feature = "server")]
#[derive(Debug)]
enum ScopeEntry {
    /// Non-distributed pattern: full limiter, never gossiped.
    /// Boxed so the enum's size is the compact tail variant's, not the
    /// ~quarter-KB limiter's — with 10⁶ tail scopes the difference is
    /// hundreds of MB.
    Local(Box<RateLimiter<f64>>),

    /// Distributed pattern below its promotion threshold: compact local
    /// enforcement of the equal share, no gossip state
    Tail { tail: TailScope, pattern_idx: usize },

    /// Distributed pattern in full gossip coordination
    Hot {
        limiter: Box<RateLimiter<f64>>,
        pattern_idx: usize,
        demotion: DemotionTracker,
    },
}

/// Multi-scope rate limit manager
#[cfg(feature = "server")]
pub struct RateLimitManager {
    /// Active scopes by name (tiered: local / tail / hot)
    scopes: HashMap<String, ScopeEntry>,

    /// Pattern configurations. Append-only so `pattern_idx` stored in scope
    /// entries stays valid; `match_order` holds the active patterns in
    /// priority order (exact > longest wildcard prefix > `*`). A pattern
    /// replaced by `set_default_pattern` keeps its slot but leaves
    /// `match_order`.
    patterns: Vec<ScopePattern>,
    match_order: Vec<usize>,

    /// Per-pattern tail aggregate (summed accepted rate of unpromoted
    /// scopes), maintained incrementally on the tail admit path — one
    /// gossiped number per pattern keeps service-level totals visible
    /// without gossiping tail scopes. Indexed like `patterns`.
    tail_windows: Vec<RateWindow>,

    /// Latest age-weighted per-pattern tail aggregates from live peers
    /// (stamped by the sync loop; reporting/visibility only)
    cluster_tail_rates: HashMap<String, f64>,

    /// Number of scopes currently in the hot tier (gossiped)
    hot_count: usize,

    /// Live peer count from the node-level gossip membership view, stamped
    /// by the sync loop each tick; tail scopes derive their equal share
    /// (`target / (1 + live_peers)`) from this
    live_peers: usize,

    /// Promotion admission floor: when the hot tier is at its gossip
    /// budget, only scopes whose estimated utilization exceeds the current
    /// minimum hot-tier utilization may promote (otherwise promotion and
    /// budget eviction would thrash the same scopes every sync). Zero when
    /// under budget. Refreshed by the sync loop.
    promotion_floor: f64,

    /// Per-node hard cap on gossiped scopes (see `TierConfig::gossip_budget`)
    gossip_budget: usize,

    /// Default target rate for new scopes (stored for future use)
    #[allow(dead_code)]
    default_target_rate: f64,

    /// Default PID parameters (stored for future use)
    #[allow(dead_code)]
    default_kp: f64,
    #[allow(dead_code)]
    default_ki: f64,
    #[allow(dead_code)]
    default_kd: f64,

    /// Gossip timing used to parameterize engine staleness/liveness;
    /// defaults to the production defaults, overridden from `Config` at
    /// startup via `set_gossip_timing`
    sync_interval: Duration,
    stale_timeout: Duration,

    /// Idle-scope TTL (see `tier::DEFAULT_SCOPE_TTL` for the derivation);
    /// sweeps run every TTL/2 from the sync-loop apply pass
    scope_ttl: Duration,
    last_ttl_sweep: Instant,
}

#[cfg(feature = "server")]
impl RateLimitManager {
    /// Create a new rate limit manager
    pub fn new(
        default_target_rate: f64,
        default_kp: f64,
        default_ki: f64,
        default_kd: f64,
    ) -> Self {
        RateLimitManager {
            scopes: HashMap::new(),
            patterns: vec![ScopePattern::default_pattern(default_target_rate)],
            match_order: vec![0],
            tail_windows: vec![RateWindow::new(Instant::now())],
            cluster_tail_rates: HashMap::new(),
            hot_count: 0,
            live_peers: 0,
            promotion_floor: 0.0,
            gossip_budget: TierConfig::default().gossip_budget,
            default_target_rate,
            default_kp,
            default_ki,
            default_kd,
            // Production defaults (Config::from_env); see set_gossip_timing
            sync_interval: Duration::from_millis(500),
            stale_timeout: Duration::from_secs(10),
            scope_ttl: crate::gossip::tier::DEFAULT_SCOPE_TTL,
            last_ttl_sweep: Instant::now(),
        }
    }

    /// Set the idle-scope TTL. Call once at startup.
    pub fn set_scope_ttl(&mut self, ttl: Duration) {
        self.scope_ttl = ttl.max(Duration::from_millis(1));
    }

    /// Align engine staleness/liveness horizons with the configured gossip
    /// timing. Call once at startup, before any scopes are created.
    pub fn set_gossip_timing(&mut self, sync_interval: Duration, stale_timeout: Duration) {
        self.sync_interval = sync_interval;
        self.stale_timeout = stale_timeout;
    }

    /// Set the per-node hard cap on gossiped (hot-tier) scopes. Call once
    /// at startup.
    pub fn set_gossip_budget(&mut self, budget: usize) {
        self.gossip_budget = budget.max(1);
    }

    /// Re-sort `match_order` by specificity (exact > longest wildcard
    /// prefix > `*`). `patterns` itself is append-only so the
    /// `pattern_idx` stored in scope entries stays valid.
    fn sort_match_order(&mut self) {
        let patterns = &self.patterns;
        self.match_order.sort_by(|&ia, &ib| {
            let a = &patterns[ia];
            let b = &patterns[ib];
            let a_exact = !a.pattern.contains('*');
            let b_exact = !b.pattern.contains('*');
            match (a_exact, b_exact) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => {
                    let a_len = a.pattern.trim_end_matches('*').len();
                    let b_len = b.pattern.trim_end_matches('*').len();
                    b_len.cmp(&a_len)
                }
            }
        });
    }

    /// Add a pattern configuration
    pub fn add_pattern(&mut self, pattern: ScopePattern) {
        self.patterns.push(pattern);
        self.tail_windows.push(RateWindow::new(Instant::now()));
        self.match_order.push(self.patterns.len() - 1);
        self.sort_match_order();
    }

    /// Replace the default catch-all pattern
    pub fn set_default_pattern(&mut self, pattern: ScopePattern) {
        // Deactivate existing "*" patterns (their slots stay so stored
        // pattern indices remain valid)
        let patterns = &self.patterns;
        self.match_order.retain(|&i| patterns[i].pattern != "*");
        self.patterns.push(pattern);
        self.tail_windows.push(RateWindow::new(Instant::now()));
        self.match_order.push(self.patterns.len() - 1);
        self.sort_match_order();
    }

    /// Find the best matching pattern index for a scope
    fn match_pattern_idx(&self, scope: &str) -> usize {
        for &idx in &self.match_order {
            if self.patterns[idx].matches(scope) != PatternMatch::None {
                return idx;
            }
        }
        // Should never happen (default * pattern always matches)
        *self
            .match_order
            .last()
            .expect("at least one active pattern")
    }

    /// Find the best matching pattern for a scope
    fn match_pattern(&self, scope: &str) -> &ScopePattern {
        &self.patterns[self.match_pattern_idx(scope)]
    }

    /// Create a rate limiter from a pattern
    fn create_limiter_from_pattern(&self, pattern: &ScopePattern) -> RateLimiter<f64> {
        self.limiter_builder(pattern).build()
    }

    /// Shared builder setup for full (Local/Hot) limiters
    fn limiter_builder(&self, pattern: &ScopePattern) -> RateLimiterBuilder<f64> {
        let pid = PIDControllerBuilder::new(pattern.target_rate)
            .kp(pattern.get_kp())
            .ki(pattern.get_ki())
            .kd(pattern.get_kd())
            .error_limit(pattern.get_error_limit())
            .build();

        let mut builder = RateLimiterBuilder::new(pattern.target_rate)
            .min_rate(pattern.get_min_rate())
            .max_rate(pattern.get_max_rate());

        builder = match pattern.engine {
            EngineKind::Pid => builder
                .engine(PidEngine::new(pid).with_staleness(self.sync_interval, self.stale_timeout)),
            EngineKind::Bayesian => builder.engine(BayesianEngine::new(
                pattern.estimator_params(self.stale_timeout),
            )),
            EngineKind::Hybrid => builder.engine(HybridEngine::new(
                pid,
                pattern.estimator_params(self.stale_timeout),
            )),
        };

        // If distributed mode, set cluster target
        if pattern.distributed {
            builder = builder.cluster_target(pattern.target_rate);
        }

        builder
    }

    /// Build the full limiter for a scope promoted out of the tail tier:
    /// refill starts at the already-enforced equal share (no one-interval
    /// burst at the cluster target) and the tail bucket's token balance
    /// carries over, so admission is continuous across the tier switch.
    fn create_promoted_limiter(
        &self,
        pattern: &ScopePattern,
        share: f64,
        tail_tokens: f64,
        now: Instant,
    ) -> RateLimiter<f64> {
        let mut builder = self
            .limiter_builder(pattern)
            .initial_timestamp(now)
            .initial_refill_rate(share);
        if share > 0.0 {
            builder = builder.initial_tokens_frac((tail_tokens / share).clamp(0.0, 1.0));
        }
        builder.build()
    }

    /// Create the initial entry for a scope: distributed patterns start in
    /// the compact tail tier, non-distributed patterns get a full local
    /// limiter (single-node control, never gossiped).
    fn create_entry(&self, pattern_idx: usize, now: Instant) -> ScopeEntry {
        let pattern = &self.patterns[pattern_idx];
        if pattern.distributed {
            let share = pattern.target_rate / (1 + self.live_peers) as f64;
            ScopeEntry::Tail {
                tail: TailScope::new(now, share),
                pattern_idx,
            }
        } else {
            ScopeEntry::Local(Box::new(self.create_limiter_from_pattern(pattern)))
        }
    }

    /// Check if a request should be throttled
    ///
    /// Auto-creates the scope if it doesn't exist.
    pub fn should_throttle(&mut self, scope: &str) -> ThrottleDecision {
        self.should_throttle_at(scope, Instant::now())
    }

    /// Check if a request should be throttled at a specific time
    pub fn should_throttle_at(&mut self, scope: &str, now: Instant) -> ThrottleDecision {
        if !self.scopes.contains_key(scope) {
            let idx = self.match_pattern_idx(scope);
            let entry = self.create_entry(idx, now);
            self.scopes.insert(scope.to_string(), entry);
        }

        // Tail tier: run the promotion test, then either promote into the
        // hot tier (and fall through to the full-limiter path) or enforce
        // the equal share locally
        if let Some(&ScopeEntry::Tail { pattern_idx, .. }) = self.scopes.get(scope) {
            let pattern = &self.patterns[pattern_idx];
            let tier_cfg = pattern.tier_config(self.gossip_budget);
            let limit = pattern.target_rate;
            let num_nodes = 1 + self.live_peers;
            let share = limit / num_nodes as f64;

            let (promote, tail_tokens, local_rate_before) = {
                let Some(ScopeEntry::Tail { tail, .. }) = self.scopes.get_mut(scope) else {
                    unreachable!("checked above");
                };
                let local_rate = tail.local_rate(now);
                let wants_promotion = should_promote(local_rate, num_nodes, limit, &tier_cfg);
                // When the hot tier is at its budget, only scopes hotter
                // than the current floor may promote (see promotion_floor)
                let admitted = self.hot_count < self.gossip_budget
                    || (local_rate * num_nodes as f64 / limit) >= self.promotion_floor;
                (wants_promotion && admitted, tail.tokens(), local_rate)
            };

            if promote {
                let limiter = self.create_promoted_limiter(
                    &self.patterns[pattern_idx],
                    share,
                    tail_tokens,
                    now,
                );
                tracing::debug!(
                    scope,
                    local_rate = local_rate_before,
                    num_nodes,
                    "promoting scope to hot tier"
                );
                self.scopes.insert(
                    scope.to_string(),
                    ScopeEntry::Hot {
                        limiter: Box::new(limiter),
                        pattern_idx,
                        demotion: DemotionTracker::default(),
                    },
                );
                self.hot_count += 1;
                // fall through to the full-limiter path below
            } else {
                let Some(ScopeEntry::Tail { tail, .. }) = self.scopes.get_mut(scope) else {
                    unreachable!("checked above");
                };
                let admitted = tail.try_admit(now, share);
                let local_accepted_rate = tail.local_rate(now);
                if admitted {
                    // Maintain the per-pattern tail aggregate incrementally
                    // (a sync-time scan over the tail set would defeat the
                    // compact-tier design)
                    self.tail_windows[pattern_idx].record(now);
                }
                return ThrottleDecision {
                    should_throttle: !admitted,
                    local_accepted_rate,
                    refill_rate: share,
                    num_peers: self.live_peers,
                };
            }
        }

        let (ScopeEntry::Local(limiter) | ScopeEntry::Hot { limiter, .. }) =
            self.scopes.get_mut(scope).expect("entry exists")
        else {
            unreachable!("tail entries handled above");
        };

        let should_throttle = limiter.should_throttle_at(now);
        let local_accepted_rate = limiter.local_accepted_request_rate();

        ThrottleDecision {
            should_throttle,
            local_accepted_rate,
            refill_rate: limiter.refill_rate(),
            num_peers: limiter.num_peers(),
        }
    }

    /// Get all active scopes
    pub fn get_all_scopes(&self) -> Vec<(String, ScopeStats)> {
        self.scopes
            .iter()
            .map(|(name, entry)| {
                let (pattern, tier) = match entry {
                    ScopeEntry::Local(_) => (self.match_pattern(name), "local"),
                    ScopeEntry::Tail { pattern_idx, .. } => (&self.patterns[*pattern_idx], "tail"),
                    ScopeEntry::Hot { pattern_idx, .. } => (&self.patterns[*pattern_idx], "hot"),
                };
                let (local_accepted_rate, num_peers) = match entry {
                    ScopeEntry::Local(l) => (l.local_accepted_request_rate(), l.num_peers()),
                    ScopeEntry::Tail { tail, .. } => {
                        (tail.local_rate_at(Instant::now()), self.live_peers)
                    }
                    ScopeEntry::Hot { limiter, .. } => {
                        (limiter.local_accepted_request_rate(), limiter.num_peers())
                    }
                };
                (
                    name.clone(),
                    ScopeStats {
                        name: name.clone(),
                        target_rate: pattern.target_rate,
                        local_accepted_rate,
                        num_peers,
                        distributed: pattern.distributed,
                        tier: tier.to_string(),
                    },
                )
            })
            .collect()
    }

    /// Get read-only reference to a scope's full limiter (`None` for
    /// tail-tier scopes, which have no limiter)
    pub fn get_limiter(&self, scope: &str) -> Option<&RateLimiter<f64>> {
        match self.scopes.get(scope)? {
            ScopeEntry::Local(l) | ScopeEntry::Hot { limiter: l, .. } => Some(l),
            ScopeEntry::Tail { .. } => None,
        }
    }

    /// Get mutable reference to a scope's full limiter (`None` for
    /// tail-tier scopes)
    pub fn get_limiter_mut(&mut self, scope: &str) -> Option<&mut RateLimiter<f64>> {
        match self.scopes.get_mut(scope)? {
            ScopeEntry::Local(l) | ScopeEntry::Hot { limiter: l, .. } => Some(l),
            ScopeEntry::Tail { .. } => None,
        }
    }

    /// Iterate mutably over all full limiters (local + hot tiers) with
    /// their scope names
    pub fn limiters_mut(&mut self) -> impl Iterator<Item = (&String, &mut RateLimiter<f64>)> {
        self.scopes
            .iter_mut()
            .filter_map(|(name, entry)| match entry {
                ScopeEntry::Local(l) | ScopeEntry::Hot { limiter: l, .. } => {
                    Some((name, l.as_mut()))
                }
                ScopeEntry::Tail { .. } => None,
            })
    }

    /// Get the number of active scopes
    pub fn num_scopes(&self) -> usize {
        self.scopes.len()
    }

    /// Number of scopes currently in the hot (gossiped) tier
    pub fn num_hot_scopes(&self) -> usize {
        self.hot_count
    }

    /// Node-level live peer count (stamped by the sync loop)
    pub fn live_peers(&self) -> usize {
        self.live_peers
    }

    /// Read-only snapshot of a scope's state for stats/debug endpoints,
    /// valid for every tier (tail scopes report their equal share as the
    /// refill rate and the node-level peer count).
    pub fn scope_snapshot(&self, scope: &str, now: Instant) -> Option<ScopeSnapshot> {
        Some(match self.scopes.get(scope)? {
            ScopeEntry::Local(l) => ScopeSnapshot {
                target_rate: l.target_rate(),
                local_accepted_rate: l.local_accepted_request_rate(),
                refill_rate: l.refill_rate(),
                num_peers: l.num_peers(),
                external_rate: l.external_accepted_request_rate(),
                tier: "local",
            },
            ScopeEntry::Tail { tail, pattern_idx } => {
                let target = self.patterns[*pattern_idx].target_rate;
                ScopeSnapshot {
                    target_rate: target,
                    local_accepted_rate: tail.local_rate_at(now),
                    refill_rate: target / (1 + self.live_peers) as f64,
                    num_peers: self.live_peers,
                    external_rate: 0.0,
                    tier: "tail",
                }
            }
            ScopeEntry::Hot { limiter, .. } => ScopeSnapshot {
                target_rate: limiter.target_rate(),
                local_accepted_rate: limiter.local_accepted_request_rate(),
                refill_rate: limiter.refill_rate(),
                num_peers: limiter.num_peers(),
                external_rate: limiter.external_accepted_request_rate(),
                tier: "hot",
            },
        })
    }

    /// Coordination tier of a scope (`"local"`, `"tail"`, or `"hot"`), or
    /// `None` if the scope doesn't exist yet
    pub fn scope_tier(&self, scope: &str) -> Option<&'static str> {
        Some(match self.scopes.get(scope)? {
            ScopeEntry::Local(_) => "local",
            ScopeEntry::Tail { .. } => "tail",
            ScopeEntry::Hot { .. } => "hot",
        })
    }

    /// One sync-tick pass applying fresh peer observations: stamps the
    /// node-level live-peer count, feeds hot limiters their per-peer
    /// observations (zeroing scopes no live peer reports), promotes tail
    /// scopes that peers gossip (coordination requires every node carrying
    /// a scope to publish its local rate), runs demotion hysteresis, and
    /// enforces the gossip budget. Call before [`collect_gossip_state`]
    /// each sync tick.
    ///
    /// [`collect_gossip_state`]: Self::collect_gossip_state
    pub fn apply_peer_observations(
        &mut self,
        observations: &[PeerObservation],
        aggregated: &AggregatedRates,
        now: Instant,
    ) {
        self.live_peers = aggregated.live_peers;
        self.cluster_tail_rates = aggregated.tail_rates.clone();

        // 1. Peer-triggered promotion: a peer gossiping a scope we hold in
        // the tail tier means the scope is hot somewhere — promote so our
        // local rate joins the coordination round.
        let peer_scopes: Vec<String> = aggregated
            .scope_rates
            .keys()
            .filter(|s| matches!(self.scopes.get(*s), Some(ScopeEntry::Tail { .. })))
            .cloned()
            .collect();
        for scope in peer_scopes {
            let Some(&ScopeEntry::Tail { pattern_idx, .. }) = self.scopes.get(&scope) else {
                continue;
            };
            let share = self.patterns[pattern_idx].target_rate / (1 + self.live_peers) as f64;
            let Some(ScopeEntry::Tail { tail, .. }) = self.scopes.get_mut(&scope) else {
                continue;
            };
            let tokens = tail.tokens();
            let limiter =
                self.create_promoted_limiter(&self.patterns[pattern_idx], share, tokens, now);
            tracing::debug!(scope, "promoting scope to hot tier (gossiped by peer)");
            self.scopes.insert(
                scope,
                ScopeEntry::Hot {
                    limiter: Box::new(limiter),
                    pattern_idx,
                    demotion: DemotionTracker::default(),
                },
            );
            self.hot_count += 1;
        }

        // 2. Feed hot limiters + demotion hysteresis
        let mut demote: Vec<String> = Vec::new();
        let mut utilizations: Vec<(String, f64)> = Vec::with_capacity(self.hot_count);
        for (name, entry) in self.scopes.iter_mut() {
            let ScopeEntry::Hot {
                limiter,
                pattern_idx,
                demotion,
            } = entry
            else {
                continue;
            };
            let external = aggregated.scope_rates.get(name).copied().unwrap_or(0.0);
            limiter.set_external_accepted_request_rate(external);
            limiter.set_num_peers(aggregated.live_peers);
            let obs: Vec<crate::engine::PeerRate<f64>> = observations
                .iter()
                .filter_map(|o| {
                    o.scope_rates.get(name).map(|rate| crate::engine::PeerRate {
                        id: o.node_id.clone(),
                        rate: *rate,
                        age: o.age,
                    })
                })
                .collect();
            limiter.set_peer_observations(obs);

            let pattern = &self.patterns[*pattern_idx];
            let cluster_rate = limiter.local_accepted_request_rate() + external;
            let tier_cfg = pattern.tier_config(self.gossip_budget);
            if demotion.observe(cluster_rate, pattern.target_rate, now, &tier_cfg) {
                demote.push(name.clone());
            } else {
                utilizations.push((name.clone(), cluster_rate / pattern.target_rate));
            }
        }

        for name in &demote {
            self.demote_scope(name, now, "sustained low utilization");
        }

        // 3. Gossip budget: evict lowest-utilization hot scopes on overflow
        if self.hot_count > self.gossip_budget {
            let evictions = budget_evictions(utilizations.clone(), self.gossip_budget);
            tracing::warn!(
                evicted = evictions.len(),
                budget = self.gossip_budget,
                "gossip budget overflow: evicting lowest-utilization scopes to tail tier"
            );
            let evicted: std::collections::HashSet<&String> = evictions.iter().collect();
            utilizations.retain(|(name, _)| !evicted.contains(name));
            for name in &evictions {
                self.demote_scope(name, now, "gossip budget eviction");
            }
        }

        // 4. Refresh the promotion admission floor (min utilization among
        // the scopes that *kept* their slots)
        // 5. TTL sweep: evict idle scopes (tail scopes are behaviorally
        // lossless to evict once idle past the estimator window; local
        // scopes lose only a long-stale control state; hot scopes demote
        // first and age out as tail)
        if now.saturating_duration_since(self.last_ttl_sweep) >= self.scope_ttl / 2 {
            self.last_ttl_sweep = now;
            let ttl = self.scope_ttl;
            let before = self.scopes.len();
            self.scopes.retain(|_, entry| match entry {
                ScopeEntry::Tail { tail, .. } => {
                    now.saturating_duration_since(tail.last_activity()) < ttl
                }
                ScopeEntry::Local(limiter) => limiter
                    .last_accept_at()
                    .map(|at| now.saturating_duration_since(at) < ttl)
                    .unwrap_or(false),
                ScopeEntry::Hot { .. } => true,
            });
            let evicted = before - self.scopes.len();
            if evicted > 0 {
                tracing::debug!(
                    evicted,
                    remaining = self.scopes.len(),
                    "TTL-evicted idle scopes"
                );
                if evicted > self.scopes.len() {
                    self.scopes.shrink_to_fit();
                }
            }
        }

        self.promotion_floor = if self.hot_count >= self.gossip_budget {
            let floor = utilizations
                .iter()
                .map(|(_, u)| *u)
                .fold(f64::INFINITY, f64::min);
            if floor.is_finite() {
                floor.max(0.0)
            } else {
                0.0
            }
        } else {
            0.0
        };
    }

    /// Demote a hot scope back to the tail tier, carrying its token balance
    fn demote_scope(&mut self, scope: &str, now: Instant, reason: &str) {
        let Some(ScopeEntry::Hot {
            limiter,
            pattern_idx,
            ..
        }) = self.scopes.get(scope)
        else {
            return;
        };
        let pattern_idx = *pattern_idx;
        let share = self.patterns[pattern_idx].target_rate / (1 + self.live_peers) as f64;
        let tokens = limiter.tokens().min(share);
        tracing::debug!(scope, reason, "demoting scope to tail tier");
        self.scopes.insert(
            scope.to_string(),
            ScopeEntry::Tail {
                tail: TailScope::with_tokens(now, tokens),
                pattern_idx,
            },
        );
        self.hot_count -= 1;
    }

    /// Refresh limiter state and collect the gossip payload: per-scope
    /// rates for hot-tier scopes plus the per-pattern tail aggregates
    /// (summed unpromoted rates — tail scopes themselves never gossip;
    /// non-distributed scopes never participate). Also ticks local
    /// limiters so their control loops stay current under zero load.
    #[allow(clippy::type_complexity)]
    pub fn collect_gossip_rates(
        &mut self,
        now: Instant,
    ) -> (Vec<(String, f64)>, Vec<(String, f64)>) {
        let mut rates = Vec::with_capacity(self.hot_count);
        for (name, entry) in self.scopes.iter_mut() {
            match entry {
                ScopeEntry::Local(limiter) => {
                    limiter.update_state_at(now);
                }
                ScopeEntry::Hot { limiter, .. } => {
                    limiter.update_state_at(now);
                    rates.push((name.clone(), limiter.local_accepted_request_rate()));
                }
                ScopeEntry::Tail { .. } => {}
            }
        }
        // One aggregate per active distributed pattern
        let mut tail_rates = Vec::new();
        for &idx in &self.match_order {
            if self.patterns[idx].distributed {
                tail_rates.push((
                    self.patterns[idx].pattern.clone(),
                    self.tail_windows[idx].rate(now),
                ));
            }
        }
        (rates, tail_rates)
    }

    /// Latest age-weighted per-pattern tail aggregates from live peers
    /// (visibility only — stamped by the sync loop each tick)
    pub fn cluster_tail_rates(&self) -> &HashMap<String, f64> {
        &self.cluster_tail_rates
    }

    /// This node's per-pattern tail aggregate (summed accepted rate of
    /// unpromoted scopes matching the pattern)
    pub fn local_tail_rate(&self, pattern: &str, now: Instant) -> f64 {
        self.match_order
            .iter()
            .find(|&&idx| self.patterns[idx].pattern == pattern)
            .map(|&idx| self.tail_windows[idx].rate_at(now))
            .unwrap_or(0.0)
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn test_pattern_exact_match() {
        let pattern = ScopePattern {
            pattern: "api#premium".to_string(),
            target_rate: 1000.0,
            min_rate: None,
            max_rate: None,
            kp: None,
            ki: None,
            kd: None,
            error_limit_frac: None,
            distributed: false,
            engine: EngineKind::Pid,
            process_noise: None,
            measurement_noise: None,
            confidence_z: None,
            promote_utilization: None,
            demote_utilization: None,
            demote_hold_secs: None,
        };

        assert_eq!(pattern.matches("api#premium"), PatternMatch::Exact);
        assert_eq!(pattern.matches("api#basic"), PatternMatch::None);
        assert_eq!(pattern.matches("api#premium_123"), PatternMatch::None);
    }

    #[test]
    #[serial]
    fn test_pattern_wildcard_match() {
        let pattern = ScopePattern {
            pattern: "api#*".to_string(),
            target_rate: 100.0,
            min_rate: None,
            max_rate: None,
            kp: None,
            ki: None,
            kd: None,
            error_limit_frac: None,
            distributed: false,
            engine: EngineKind::Pid,
            process_noise: None,
            measurement_noise: None,
            confidence_z: None,
            promote_utilization: None,
            demote_utilization: None,
            demote_hold_secs: None,
        };

        assert_eq!(pattern.matches("api#premium"), PatternMatch::Wildcard);
        assert_eq!(pattern.matches("api#basic"), PatternMatch::Wildcard);
        assert_eq!(pattern.matches("api#"), PatternMatch::Wildcard);
        assert_eq!(pattern.matches("web#page"), PatternMatch::None);
    }

    #[test]
    #[serial]
    fn test_pattern_defaults() {
        let pattern = ScopePattern::default_pattern(100.0);

        assert_eq!(pattern.get_min_rate(), 50.0);
        assert_eq!(pattern.get_max_rate(), 200.0);
        assert_eq!(pattern.get_kp(), 0.8);
        assert_eq!(pattern.get_ki(), 0.05);
        assert_eq!(pattern.get_kd(), 0.04);
    }

    #[test]
    #[serial]
    fn test_manager_default_scope() {
        let mut manager = RateLimitManager::new(100.0, 0.8, 0.05, 0.04);

        // First request should not throttle
        let decision = manager.should_throttle("test-scope");
        assert!(!decision.should_throttle);
        assert_eq!(manager.num_scopes(), 1);
    }

    #[test]
    #[serial]
    fn test_manager_auto_creation() {
        let mut manager = RateLimitManager::new(100.0, 0.8, 0.05, 0.04);

        // Create multiple scopes
        manager.should_throttle("scope-1");
        manager.should_throttle("scope-2");
        manager.should_throttle("scope-3");

        assert_eq!(manager.num_scopes(), 3);
    }

    #[test]
    #[serial]
    fn test_manager_pattern_priority_exact_match() {
        let mut manager = RateLimitManager::new(10.0, 0.8, 0.05, 0.04);

        // Add patterns (order doesn't matter, they'll be sorted)
        manager.add_pattern(ScopePattern {
            pattern: "api#*".to_string(),
            target_rate: 100.0,
            min_rate: None,
            max_rate: None,
            kp: None,
            ki: None,
            kd: None,
            error_limit_frac: None,
            distributed: false,
            engine: EngineKind::Pid,
            process_noise: None,
            measurement_noise: None,
            confidence_z: None,
            promote_utilization: None,
            demote_utilization: None,
            demote_hold_secs: None,
        });

        manager.add_pattern(ScopePattern {
            pattern: "api#premium".to_string(),
            target_rate: 1000.0,
            min_rate: None,
            max_rate: None,
            kp: None,
            ki: None,
            kd: None,
            error_limit_frac: None,
            distributed: false,
            engine: EngineKind::Pid,
            process_noise: None,
            measurement_noise: None,
            confidence_z: None,
            promote_utilization: None,
            demote_utilization: None,
            demote_hold_secs: None,
        });

        // Exact match should take priority
        manager.should_throttle("api#premium");
        let limiter = manager.get_limiter("api#premium").unwrap();
        assert_eq!(limiter.target_rate(), 1000.0);

        // Wildcard match
        manager.should_throttle("api#basic");
        let limiter = manager.get_limiter("api#basic").unwrap();
        assert_eq!(limiter.target_rate(), 100.0);

        // Default pattern
        manager.should_throttle("web#page");
        let limiter = manager.get_limiter("web#page").unwrap();
        assert_eq!(limiter.target_rate(), 10.0);
    }

    #[test]
    #[serial]
    fn test_manager_pattern_priority_most_specific() {
        let mut manager = RateLimitManager::new(10.0, 0.8, 0.05, 0.04);

        manager.add_pattern(ScopePattern {
            pattern: "api#*".to_string(),
            target_rate: 100.0,
            min_rate: None,
            max_rate: None,
            kp: None,
            ki: None,
            kd: None,
            error_limit_frac: None,
            distributed: false,
            engine: EngineKind::Pid,
            process_noise: None,
            measurement_noise: None,
            confidence_z: None,
            promote_utilization: None,
            demote_utilization: None,
            demote_hold_secs: None,
        });

        manager.add_pattern(ScopePattern {
            pattern: "api#premium_*".to_string(),
            target_rate: 500.0,
            min_rate: None,
            max_rate: None,
            kp: None,
            ki: None,
            kd: None,
            error_limit_frac: None,
            distributed: false,
            engine: EngineKind::Pid,
            process_noise: None,
            measurement_noise: None,
            confidence_z: None,
            promote_utilization: None,
            demote_utilization: None,
            demote_hold_secs: None,
        });

        // Most specific wildcard should match
        manager.should_throttle("api#premium_user123");
        let limiter = manager.get_limiter("api#premium_user123").unwrap();
        assert_eq!(limiter.target_rate(), 500.0);

        // Less specific wildcard
        manager.should_throttle("api#basic");
        let limiter = manager.get_limiter("api#basic").unwrap();
        assert_eq!(limiter.target_rate(), 100.0);
    }

    #[test]
    #[serial]
    fn test_manager_get_all_scopes() {
        let mut manager = RateLimitManager::new(100.0, 0.8, 0.05, 0.04);

        manager.should_throttle("scope-1");
        manager.should_throttle("scope-2");
        manager.should_throttle("scope-3");

        let scopes = manager.get_all_scopes();
        assert_eq!(scopes.len(), 3);

        let names: Vec<String> = scopes.iter().map(|(name, _)| name.clone()).collect();
        assert!(names.contains(&"scope-1".to_string()));
        assert!(names.contains(&"scope-2".to_string()));
        assert!(names.contains(&"scope-3".to_string()));
    }

    #[test]
    #[serial]
    fn test_manager_get_limiter_mut() {
        let mut manager = RateLimitManager::new(100.0, 0.8, 0.05, 0.04);

        manager.should_throttle("test-scope");

        // Get mutable reference and update external rate
        if let Some(limiter) = manager.get_limiter_mut("test-scope") {
            limiter.set_external_accepted_request_rate(50.0);
            limiter.set_num_peers(2);
        }

        let decision = manager.should_throttle("test-scope");
        assert_eq!(decision.num_peers, 2);
    }

    #[test]
    #[serial]
    fn test_manager_throttle_decision_fields() {
        let mut manager = RateLimitManager::new(100.0, 0.8, 0.05, 0.04);

        let decision = manager.should_throttle("test");
        assert!(!decision.should_throttle);
        assert!(decision.local_accepted_rate >= 0.0);
        assert!(decision.refill_rate > 0.0);
        assert_eq!(decision.num_peers, 0);
    }
}
