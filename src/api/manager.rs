//! Multi-scope rate limit manager
//!
//! Manages multiple rate limiters with pattern-based configuration and auto-creation.

use crate::engine::{BayesianEngine, BayesianParams, EngineKind, HybridEngine, PidEngine};
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
        }
    }

    /// Estimator parameters for the bayesian/hybrid engines, with the
    /// membership horizon aligned to the transport's stale timeout.
    fn estimator_params(&self, stale_timeout: Duration) -> BayesianParams {
        let defaults = BayesianParams::default();
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
}

/// Multi-scope rate limit manager
#[cfg(feature = "server")]
pub struct RateLimitManager {
    /// Active rate limiters by scope name
    limiters: HashMap<String, RateLimiter<f64>>,

    /// Pattern configurations (ordered by priority)
    patterns: Vec<ScopePattern>,

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
            limiters: HashMap::new(),
            patterns: vec![ScopePattern::default_pattern(default_target_rate)],
            default_target_rate,
            default_kp,
            default_ki,
            default_kd,
            // Production defaults (Config::from_env); see set_gossip_timing
            sync_interval: Duration::from_millis(500),
            stale_timeout: Duration::from_secs(10),
        }
    }

    /// Align engine staleness/liveness horizons with the configured gossip
    /// timing. Call once at startup, before any scopes are created.
    pub fn set_gossip_timing(&mut self, sync_interval: Duration, stale_timeout: Duration) {
        self.sync_interval = sync_interval;
        self.stale_timeout = stale_timeout;
    }

    /// Add a pattern configuration
    pub fn add_pattern(&mut self, pattern: ScopePattern) {
        self.patterns.push(pattern);
        // Sort patterns by specificity (exact > wildcard)
        self.patterns.sort_by(|a, b| {
            // Exact matches first
            let a_exact = !a.pattern.contains('*');
            let b_exact = !b.pattern.contains('*');
            match (a_exact, b_exact) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => {
                    // Among wildcards, longer prefix is more specific
                    let a_len = a.pattern.trim_end_matches('*').len();
                    let b_len = b.pattern.trim_end_matches('*').len();
                    b_len.cmp(&a_len)
                }
            }
        });
    }

    /// Replace the default catch-all pattern
    pub fn set_default_pattern(&mut self, pattern: ScopePattern) {
        // Remove any existing "*" wildcard patterns
        self.patterns.retain(|p| p.pattern != "*");
        // Add the new default pattern
        self.patterns.push(pattern);
        // Re-sort
        self.patterns.sort_by(|a, b| {
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

    /// Find the best matching pattern for a scope
    fn match_pattern(&self, scope: &str) -> &ScopePattern {
        for pattern in &self.patterns {
            if pattern.matches(scope) != PatternMatch::None {
                return pattern;
            }
        }
        // Should never happen (default * pattern always matches)
        &self.patterns[self.patterns.len() - 1]
    }

    /// Create a rate limiter from a pattern
    fn create_limiter_from_pattern(&self, pattern: &ScopePattern) -> RateLimiter<f64> {
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

        builder.build()
    }

    /// Check if a request should be throttled
    ///
    /// Auto-creates the scope if it doesn't exist.
    pub fn should_throttle(&mut self, scope: &str) -> ThrottleDecision {
        // Check if limiter exists, if not create it
        if !self.limiters.contains_key(scope) {
            let pattern = self.match_pattern(scope);
            let limiter = self.create_limiter_from_pattern(pattern);
            self.limiters.insert(scope.to_string(), limiter);
        }

        // Now we can borrow the limiter mutably
        let limiter = self.limiters.get_mut(scope).unwrap();

        let should_throttle = limiter.should_throttle();
        let local_accepted_rate = limiter.local_accepted_request_rate();

        ThrottleDecision {
            should_throttle,
            local_accepted_rate,
            refill_rate: limiter.refill_rate(),
            num_peers: limiter.num_peers(),
        }
    }

    /// Check if a request should be throttled at a specific time
    pub fn should_throttle_at(&mut self, scope: &str, now: Instant) -> ThrottleDecision {
        // Check if limiter exists, if not create it
        if !self.limiters.contains_key(scope) {
            let pattern = self.match_pattern(scope);
            let limiter = self.create_limiter_from_pattern(pattern);
            self.limiters.insert(scope.to_string(), limiter);
        }

        // Now we can borrow the limiter mutably
        let limiter = self.limiters.get_mut(scope).unwrap();

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
        self.limiters
            .iter()
            .map(|(name, limiter)| {
                let pattern = self.match_pattern(name);
                (
                    name.clone(),
                    ScopeStats {
                        name: name.clone(),
                        target_rate: pattern.target_rate,
                        local_accepted_rate: limiter.local_accepted_request_rate(),
                        num_peers: limiter.num_peers(),
                        distributed: pattern.distributed,
                    },
                )
            })
            .collect()
    }

    /// Get read-only reference to a limiter (for stats queries)
    pub fn get_limiter(&self, scope: &str) -> Option<&RateLimiter<f64>> {
        self.limiters.get(scope)
    }

    /// Get mutable reference to a limiter (for gossip updates)
    pub fn get_limiter_mut(&mut self, scope: &str) -> Option<&mut RateLimiter<f64>> {
        self.limiters.get_mut(scope)
    }

    /// Iterate mutably over all limiters with their scope names
    ///
    /// Used by the gossip sync loop to apply external rates and peer counts in
    /// a single pass under one write lock.
    pub fn limiters_mut(&mut self) -> impl Iterator<Item = (&String, &mut RateLimiter<f64>)> {
        self.limiters.iter_mut()
    }

    /// Get the number of active scopes
    pub fn num_scopes(&self) -> usize {
        self.limiters.len()
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
        });

        // Exact match should take priority
        manager.should_throttle("api#premium");
        let limiter = manager.limiters.get("api#premium").unwrap();
        assert_eq!(limiter.target_rate(), 1000.0);

        // Wildcard match
        manager.should_throttle("api#basic");
        let limiter = manager.limiters.get("api#basic").unwrap();
        assert_eq!(limiter.target_rate(), 100.0);

        // Default pattern
        manager.should_throttle("web#page");
        let limiter = manager.limiters.get("web#page").unwrap();
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
        });

        // Most specific wildcard should match
        manager.should_throttle("api#premium_user123");
        let limiter = manager.limiters.get("api#premium_user123").unwrap();
        assert_eq!(limiter.target_rate(), 500.0);

        // Less specific wildcard
        manager.should_throttle("api#basic");
        let limiter = manager.limiters.get("api#basic").unwrap();
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
