//! Bayesian rate-estimator engine: per-peer scalar Kalman filters plus
//! uncertainty-aware admission.
//!
//! # Model
//!
//! Each peer's true accepted rate is a latent variable observed through
//! delayed, noisy gossip samples. Per (peer, scope) we run a scalar Kalman
//! filter with random-walk dynamics:
//!
//! ```text
//! state:        x_k = x_{k-1} + w_k,   w_k ~ N(0, q · Δt)
//! observation:  z_k = x_k + v_k,       v_k ~ N(0, r)
//! ```
//!
//! where `q` (process noise, rps²/s) encodes how fast peer rates are
//! believed to change and `r` (measurement noise, rps²) the sampling error
//! of the peer's sliding-window rate estimate. The filter equations are the
//! standard scalar predict/update cycle (Welch & Bishop, "An Introduction to
//! the Kalman Filter", UNC-Chapel Hill TR 95-041, eqs. 1.9–1.12):
//!
//! ```text
//! predict:  x̂⁻ = x̂           (random walk: F = 1, no control input)
//!           P⁻ = P + q · Δt
//! update:   K  = P⁻ / (P⁻ + r)
//!           x̂  = x̂⁻ + K (z − x̂⁻)
//!           P  = (1 − K) P⁻
//! ```
//!
//! Between gossip samples no update runs, but the *usable* variance of a
//! peer's estimate at query time is `P + q · age`: *staleness is
//! uncertainty*. This subsumes Milestone 3's linear staleness decay with a
//! principled equivalent — an old observation doesn't fade toward zero rate,
//! it stays at its last mean with ever-wider error bars.
//!
//! # Admission policy
//!
//! The cluster estimate is the sum of independent per-peer Gaussians:
//!
//! ```text
//! Ŝ = Σ x̂ᵢ          (peer total mean)
//! V = Σ (Pᵢ + q·ageᵢ)  (peer total variance; independence assumed)
//! ```
//!
//! The node then admits against the estimate's upper confidence bound,
//! taking whatever headroom the cluster target leaves:
//!
//! ```text
//! refill = cluster_target − (Ŝ + z·√V)
//! ```
//!
//! clamped to `[min_rate/n, max_rate/n]` (n = 1 + live peers). With
//! symmetric saturated demand this fixed-points at the fair share
//! `(cluster_target − (n−1)·z·σ)/n`; under skew, cold nodes' low observed
//! rates leave headroom the hot node claims automatically — share division
//! is demand-weighted without explicit coordination. The `z·√V` term makes
//! the node conservative exactly when information is stale (partition,
//! churn) and aggressive when the estimate is tight.
//!
//! # Membership
//!
//! Peers whose observations exceed `stale_timeout` are dropped from the
//! filter bank entirely. The random-walk model keeps a silent peer's *mean*
//! at its last value forever (only the variance grows), so without a
//! membership cutoff a dead peer would permanently reserve headroom. The
//! cutoff is a transport/membership concern, deliberately identical to the
//! gossip layer's liveness horizon; estimation handles everything short
//! of it.

use std::collections::BTreeMap;
use std::time::Duration;

use num_traits::{Float, FromPrimitive, Signed};

use super::{ControlInput, RateController, DEFAULT_STALE_TIMEOUT};

/// Tuning parameters for [`BayesianEngine`] (and the estimator half of
/// [`super::HybridEngine`]).
///
/// Defaults are simulator-derived (Milestone 5.3 sweep, seed 42, recorded
/// in `docs/engine-comparison.md`). For the pure Bayesian engine the
/// estimate-and-set feedback race must be damped by a slow Kalman gain:
/// `q = 1, r = 100` was the only swept corner where most scenarios
/// converge (fast-gain settings oscillate at ±150 rps). `r = 100` is also
/// what Poisson counting noise predicts for a 1 s window at the
/// benchmark's ~100 rps per-node shares (variance ≈ λ). `z = 1` is the
/// roadmap's documented starting point; the sweep showed z mainly trades
/// undershoot for convergence speed with no clearly better setting.
///
/// The hybrid engine wants the opposite: a fast filter (its PID supplies
/// the damping) — use [`BayesianParams::hybrid_default`] there.
#[derive(Debug, Clone, Copy)]
pub struct BayesianParams {
    /// Process noise `q` (rps²/s): how fast peer rates are believed to
    /// change. Larger values track fast-moving peers more closely but keep
    /// wider error bars on stale data.
    pub process_noise: f64,

    /// Measurement noise `r` (rps²): variance of a single gossiped rate
    /// sample. For a 1 s sliding-window estimate of a Poisson stream at
    /// rate λ, the counting error alone gives variance ≈ λ, so values well
    /// below the per-node share underweight real sampling noise.
    pub measurement_noise: f64,

    /// Confidence multiplier `z` for the admission bound
    /// (`mean + z·σ`). 0 admits against the raw mean; 1 (default) against
    /// one standard deviation of headroom pessimism.
    pub confidence_z: f64,

    /// Membership horizon: peers silent longer than this are dropped from
    /// the filter bank. Should match the transport's stale timeout.
    pub stale_timeout: Duration,
}

impl Default for BayesianParams {
    fn default() -> Self {
        BayesianParams {
            process_noise: 1.0,
            measurement_noise: 100.0,
            confidence_z: 1.0,
            stale_timeout: DEFAULT_STALE_TIMEOUT,
        }
    }
}

impl BayesianParams {
    /// Estimator defaults for [`super::HybridEngine`]: a fast filter
    /// (`q = 10, r = 10`), since the PID half supplies the damping.
    /// Simulator-derived (Milestone 5.3 sweep): hybrid results are nearly
    /// identical across `q/r` within ×10 of this point, while the
    /// Bayesian-tuned slow gain (`q = 1, r = 100`) measurably slows hybrid
    /// convergence (scale_50 never re-enters the band).
    pub fn hybrid_default() -> Self {
        BayesianParams {
            process_noise: 10.0,
            measurement_noise: 10.0,
            confidence_z: 1.0,
            stale_timeout: DEFAULT_STALE_TIMEOUT,
        }
    }
}

/// One peer's scalar Kalman filter state.
#[derive(Debug, Clone)]
struct PeerFilter<T> {
    /// Posterior mean x̂ at the last incorporated sample
    mean: T,

    /// Posterior variance P at the last incorporated sample
    variance: T,

    /// Engine-clock time of the last incorporated sample (t_now − age);
    /// used both to detect new samples and to size the predict step
    last_sample_time: T,
}

/// Shared per-peer Kalman filter bank (used by both the Bayesian and hybrid
/// engines). Keyed by peer id in a `BTreeMap` so summation order — and
/// therefore floating-point results — is deterministic.
#[derive(Debug, Clone)]
pub(crate) struct FilterBank<T> {
    filters: BTreeMap<String, PeerFilter<T>>,

    /// Engine-local clock, advanced by `input.dt` each update. Only
    /// differences of this clock are ever used.
    clock: T,

    process_noise: T,
    measurement_noise: T,
    stale_timeout: Duration,
}

/// A peer's estimate evaluated at the current engine clock.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PeerEstimate<T> {
    pub mean: T,
    pub variance: T,
}

impl<T: Float + FromPrimitive + Copy> FilterBank<T> {
    pub(crate) fn new(params: &BayesianParams) -> Self {
        FilterBank {
            filters: BTreeMap::new(),
            clock: T::zero(),
            process_noise: T::from_f64(params.process_noise).unwrap(),
            measurement_noise: T::from_f64(params.measurement_noise).unwrap(),
            stale_timeout: params.stale_timeout,
        }
    }

    /// Advance the clock, incorporate new samples, prune dead peers, and
    /// return each live peer's estimate at the current time.
    pub(crate) fn observe(
        &mut self,
        peers: &[super::PeerRate<T>],
        dt: Duration,
    ) -> Vec<PeerEstimate<T>> {
        self.clock = self.clock + T::from_f64(dt.as_secs_f64()).unwrap();

        let mut estimates = Vec::with_capacity(peers.len());
        let mut seen: Vec<&str> = Vec::with_capacity(peers.len());

        for obs in peers {
            // Past the membership horizon the peer is presumed gone; skip it
            // here and prune its filter below
            if obs.age >= self.stale_timeout {
                continue;
            }
            seen.push(obs.id.as_str());

            let age = T::from_f64(obs.age.as_secs_f64()).unwrap();
            let sample_time = self.clock - age;

            let filter = self
                .filters
                .entry(obs.id.clone())
                .or_insert_with(|| PeerFilter {
                    // First contact: the sample itself is the prior, with a
                    // single measurement's worth of variance (Welch & Bishop
                    // §4: filter initialization from the first observation)
                    mean: obs.rate,
                    variance: self.measurement_noise,
                    last_sample_time: sample_time,
                });

            // A strictly newer sample time means the transport delivered a
            // fresh observation (ages reset on receipt); run one
            // predict/update cycle sized by the inter-sample gap
            let gap = sample_time - filter.last_sample_time;
            if gap > T::zero() {
                let predicted_var = filter.variance + self.process_noise * gap;
                let gain = predicted_var / (predicted_var + self.measurement_noise);
                filter.mean = filter.mean + gain * (obs.rate - filter.mean);
                filter.variance = (T::one() - gain) * predicted_var;
                filter.last_sample_time = sample_time;
            }

            // Estimate at the current clock: mean is unchanged under the
            // random walk; variance grows with time since the last sample
            let since_sample = self.clock - filter.last_sample_time;
            estimates.push(PeerEstimate {
                mean: filter.mean,
                variance: filter.variance + self.process_noise * since_sample,
            });
        }

        // Prune peers no longer observed (or past the horizon) so a dead
        // peer's mean cannot reserve headroom forever
        self.filters.retain(|id, _| seen.contains(&id.as_str()));

        estimates
    }
}

/// Pure Bayesian estimate-and-set engine. See the module docs for the
/// model, admission policy, and parameter meanings.
#[derive(Debug, Clone)]
pub struct BayesianEngine<T> {
    bank: FilterBank<T>,
    confidence_z: T,
    last_setpoint: Option<T>,
}

impl<T: Float + FromPrimitive + Copy> BayesianEngine<T> {
    pub fn new(params: BayesianParams) -> Self {
        BayesianEngine {
            bank: FilterBank::new(&params),
            confidence_z: T::from_f64(params.confidence_z).unwrap(),
            last_setpoint: None,
        }
    }
}

impl<T: Float + Signed + FromPrimitive + Copy + Send + Sync + std::fmt::Debug> RateController<T>
    for BayesianEngine<T>
{
    fn update(&mut self, input: &ControlInput<'_, T>) -> T {
        let Some(cluster_target) = input.cluster_target else {
            // Single-node mode: estimation adds nothing (there are no peers
            // to estimate); the token bucket enforces the static target
            let refill = num_traits::clamp(input.target_rate, input.min_rate, input.max_rate);
            self.last_setpoint = Some(refill);
            return refill;
        };

        let estimates = self.bank.observe(input.peers, input.dt);
        let live = estimates.len();

        let mut peer_sum = T::zero();
        let mut peer_var = T::zero();
        for e in &estimates {
            peer_sum = peer_sum + e.mean;
            peer_var = peer_var + e.variance;
        }

        // Admit against the upper confidence bound of the peer total
        let ucb = peer_sum + self.confidence_z * peer_var.sqrt();
        let headroom = cluster_target - ucb;

        let n = T::from_usize(1 + live).unwrap();
        let refill = num_traits::clamp(headroom, input.min_rate / n, input.max_rate / n);
        self.last_setpoint = Some(refill);
        refill
    }

    fn setpoint(&self) -> T {
        // The engine's effective local target is the admission rate itself
        self.last_setpoint.unwrap_or_else(T::zero)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::PeerRate;

    fn peer(id: &str, rate: f64, age_ms: u64) -> PeerRate<f64> {
        PeerRate {
            id: id.to_string(),
            rate,
            age: Duration::from_millis(age_ms),
        }
    }

    fn input<'a>(
        local_rate: f64,
        peers: &'a [PeerRate<f64>],
        cluster_target: Option<f64>,
    ) -> ControlInput<'a, f64> {
        ControlInput {
            local_rate,
            peers,
            target_rate: 100.0,
            cluster_target,
            min_rate: 30.0,
            max_rate: 600.0,
            dt: Duration::from_secs(1),
        }
    }

    #[test]
    fn test_no_peers_takes_full_target() {
        let mut engine = BayesianEngine::new(BayesianParams::default());
        let refill = engine.update(&input(0.0, &[], Some(300.0)));
        assert_eq!(refill, 300.0, "cold start: no peers → full cluster target");
    }

    #[test]
    fn test_headroom_shrinks_with_peer_load() {
        let mut engine = BayesianEngine::new(BayesianParams::default());
        let peers = vec![peer("a", 100.0, 0), peer("b", 100.0, 0)];
        let refill = engine.update(&input(100.0, &peers, Some(300.0)));
        // Headroom = 300 − (200 + z·σ) < 100, and above min bound 30/3
        assert!(refill < 100.0, "got {}", refill);
        assert!(refill > 10.0, "got {}", refill);
    }

    #[test]
    fn test_staleness_widens_error_bars() {
        let params = BayesianParams::default();
        let mut fresh_engine = BayesianEngine::new(params);
        let mut stale_engine = BayesianEngine::new(params);

        // Both engines see the same peer, then it goes silent for one
        let warm = vec![peer("a", 100.0, 0)];
        fresh_engine.update(&input(100.0, &warm, Some(300.0)));
        stale_engine.update(&input(100.0, &warm, Some(300.0)));

        let fresh = fresh_engine.update(&input(100.0, &[peer("a", 100.0, 100)], Some(300.0)));
        let stale = stale_engine.update(&input(100.0, &[peer("a", 100.0, 8000)], Some(300.0)));
        assert!(
            stale < fresh,
            "stale info must admit less: stale {} vs fresh {}",
            stale,
            fresh
        );
    }

    #[test]
    fn test_dead_peer_dropped_frees_headroom() {
        let mut engine = BayesianEngine::new(BayesianParams::default());
        let warm = vec![peer("a", 200.0, 0)];
        let before = engine.update(&input(50.0, &warm, Some(300.0)));

        // Peer past the membership horizon: filter pruned, headroom back
        let gone = vec![peer("a", 200.0, 15_000)];
        let after = engine.update(&input(50.0, &gone, Some(300.0)));
        assert!(before < 150.0);
        assert_eq!(after, 300.0, "dead peer must not reserve headroom");
    }

    #[test]
    fn test_filter_converges_to_true_rate() {
        let params = BayesianParams {
            process_noise: 1.0,
            measurement_noise: 100.0,
            ..BayesianParams::default()
        };
        let mut bank = FilterBank::<f64>::new(&params);
        // Repeated samples at 100 rps, arriving fresh every second
        let mut mean = 0.0;
        for _ in 0..30 {
            let est = bank.observe(&[peer("a", 100.0, 0)], Duration::from_secs(1));
            mean = est[0].mean;
        }
        assert!((mean - 100.0).abs() < 1.0, "converged to {}", mean);
    }

    #[test]
    fn test_deterministic_summation_order() {
        // BTreeMap keying: identical inputs in different list order give
        // identical results
        let mk = || BayesianEngine::new(BayesianParams::default());
        let (mut e1, mut e2) = (mk(), mk());
        let fwd = vec![peer("a", 10.0, 0), peer("b", 20.0, 0), peer("c", 30.0, 0)];
        let rev: Vec<_> = fwd.iter().rev().cloned().collect();
        let r1 = e1.update(&input(5.0, &fwd, Some(300.0)));
        let r2 = e2.update(&input(5.0, &rev, Some(300.0)));
        assert_eq!(r1, r2);
    }

    #[test]
    fn test_single_node_mode_static() {
        let mut engine = BayesianEngine::new(BayesianParams::default());
        let refill = engine.update(&input(50.0, &[], None));
        assert_eq!(refill, 100.0);
    }
}
