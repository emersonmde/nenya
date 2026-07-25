//! Pluggable control engines (Milestone 5).
//!
//! A [`RateController`] turns observations — the local accepted rate plus
//! per-peer `(rate, age)` samples — into a new local token refill rate. The
//! trait boundary is deliberately narrow: engines see observations, not
//! gossip internals, so the same engine runs unchanged under the Chitchat
//! transport, the deterministic simulator, and any future blackboard
//! transport. How an engine weighs staleness, divides the cluster target
//! across nodes, and reacts to noise is *inside* the boundary — aggregation
//! strategy is part of what engines compete on.
//!
//! Three engines ship:
//!
//! - [`PidEngine`] — the original equal-division PID loop, ported unchanged.
//! - [`BayesianEngine`] — per-peer scalar Kalman filters; admits against an
//!   upper confidence bound of the cluster estimate, so staleness maps to
//!   uncertainty and the node is conservative exactly when information is
//!   old.
//! - [`HybridEngine`] — the Kalman peer estimate feeding a cluster-level PID.
//!
//! The engine is always an explicit config choice — never selected at
//! runtime. See `docs/engine-comparison.md` for the benchmark data behind
//! the recommended default.

mod bayesian;
mod hybrid;
mod pid;

pub use bayesian::{BayesianEngine, BayesianParams};
pub use hybrid::{HybridEngine, HybridParams};
pub use pid::PidEngine;

use std::fmt;
use std::time::Duration;

/// One peer's most recent accepted-rate observation for a single scope.
///
/// `age` is measured on the local monotonic clock (time since the
/// observation was last seen to change) — peer wall-clock timestamps are
/// never compared across nodes, so clock skew cannot affect engines.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerRate<T> {
    /// Stable peer identity. Engines that track per-peer state (Kalman
    /// filters) key on this; it only needs to be unique and stable per peer.
    pub id: String,

    /// The peer's last reported accepted rate for this scope
    pub rate: T,

    /// Locally measured time since this observation last changed
    pub age: Duration,
}

/// Everything an engine sees at one control update.
///
/// `min_rate`/`max_rate` are the configured cluster-scale bounds. Engines
/// apply them (scaled by their own view of the live node count, which only
/// the engine knows); the caller merely sanitizes the output to a
/// non-negative finite value.
#[derive(Debug)]
pub struct ControlInput<'a, T> {
    /// Locally measured accepted rate (sliding window)
    pub local_rate: T,

    /// Per-peer observations for this scope
    pub peers: &'a [PeerRate<T>],

    /// Local target rate (single-node mode)
    pub target_rate: T,

    /// Cluster-wide target (distributed mode); `None` = single-node
    pub cluster_target: Option<T>,

    /// Configured minimum rate bound (cluster scale)
    pub min_rate: T,

    /// Configured maximum rate bound (cluster scale)
    pub max_rate: T,

    /// Time elapsed since the previous engine update
    pub dt: Duration,
}

/// A control engine: consumes one [`ControlInput`] per update interval and
/// returns the new local token refill rate.
///
/// Updates run in the sync loop cadence (roughly once per second per
/// scope), never on the per-request hot path.
pub trait RateController<T>: fmt::Debug + Send + Sync {
    /// Compute the new local refill rate from the current observations.
    fn update(&mut self, input: &ControlInput<'_, T>) -> T;

    /// The engine's current effective target for the local node (its share
    /// of the cluster target in distributed mode).
    fn setpoint(&self) -> T;
}

/// Which control engine to run.
///
/// Always an explicit configuration choice (`engine = "pid" | "bayesian" |
/// "hybrid"`); there is no runtime auto-selection. Benchmarks only decide
/// which value ships as the documented recommended default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "server", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "server", serde(rename_all = "lowercase"))]
pub enum EngineKind {
    #[default]
    Pid,
    Bayesian,
    Hybrid,
}

impl std::str::FromStr for EngineKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "pid" => Ok(EngineKind::Pid),
            "bayesian" => Ok(EngineKind::Bayesian),
            "hybrid" => Ok(EngineKind::Hybrid),
            other => Err(format!(
                "unknown engine '{}' (expected pid, bayesian, or hybrid)",
                other
            )),
        }
    }
}

impl fmt::Display for EngineKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            EngineKind::Pid => "pid",
            EngineKind::Bayesian => "bayesian",
            EngineKind::Hybrid => "hybrid",
        })
    }
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
///
/// Lives in the engine module (always compiled) because both the gossip
/// aggregation layer and the [`PidEngine`] liveness test use it.
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

/// Production default gossip sync interval (`NENYA_SYNC_INTERVAL_MS`);
/// engines that need staleness parameters default to this so a plain
/// library embedding behaves like the server defaults.
pub(crate) const DEFAULT_SYNC_INTERVAL: Duration = Duration::from_millis(500);

/// Production default stale timeout (`NENYA_STALE_TIMEOUT_MS`).
pub(crate) const DEFAULT_STALE_TIMEOUT: Duration = Duration::from_secs(10);
