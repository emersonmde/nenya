//! Hybrid engine: Kalman-filtered cluster estimate feeding a cluster-level
//! PID loop.
//!
//! The estimator half is the same per-peer filter bank as
//! [`BayesianEngine`](super::BayesianEngine) (see that module for the model
//! and citations). The control half differs from [`PidEngine`](super::PidEngine)
//! in two ways:
//!
//! 1. **The feedback signal is the estimated cluster total**
//!    (`local_rate + Σ peer means`), not the local rate alone. The filter
//!    smooths gossip sampling noise before it reaches the controller.
//! 2. **Control runs at cluster level, divided by n**: every node computes
//!    the same cluster error `cluster_target − estimate`, and applies
//!    `correction / n` around its `target / n` share. Summed over n nodes
//!    this reproduces exactly one controller's worth of aggregate response,
//!    so the loop gain — and settling time — is independent of fleet size,
//!    unlike equal-division PID where each node integrates only its `1/n`
//!    error slice. The same configured gains remain valid because the
//!    per-node output magnitude is unchanged.
//!
//! No optimality is claimed: the separation principle that would make
//! estimate-then-control provably optimal assumes a linear plant, Gaussian
//! noise, and no delay, all of which gossip coordination violates. The
//! engine earns its place (or not) in the Milestone 5 scenario matrix.

use num_traits::{Float, FromPrimitive, Signed};

use super::bayesian::FilterBank;
use super::{BayesianParams, ControlInput, RateController};
use crate::pid_controller::PIDController;

/// Estimator parameters for [`HybridEngine`]. The admission-side
/// `confidence_z` is unused here (the PID consumes the mean estimate);
/// process/measurement noise and the membership horizon apply as in
/// [`BayesianParams`].
pub type HybridParams = BayesianParams;

/// Kalman→PID hybrid. See the module docs.
#[derive(Debug, Clone)]
pub struct HybridEngine<T> {
    bank: FilterBank<T>,
    pid: PIDController<T>,
    last_setpoint: Option<T>,
}

impl<T: Float + FromPrimitive + Copy> HybridEngine<T> {
    pub fn new(pid: PIDController<T>, params: HybridParams) -> Self {
        HybridEngine {
            bank: FilterBank::new(&params),
            pid,
            last_setpoint: None,
        }
    }
}

impl<T: Float + Signed + FromPrimitive + Copy + Send + Sync + std::fmt::Debug> RateController<T>
    for HybridEngine<T>
{
    fn update(&mut self, input: &ControlInput<'_, T>) -> T {
        let Some(cluster_target) = input.cluster_target else {
            // Single-node mode: no peers to estimate; plain local PID
            self.pid.set_setpoint(input.target_rate);
            let correction = self.pid.compute_correction(input.local_rate);
            let refill = num_traits::clamp(
                input.target_rate + correction,
                input.min_rate,
                input.max_rate,
            );
            self.last_setpoint = Some(input.target_rate);
            return refill;
        };

        let estimates = self.bank.observe(input.peers, input.dt);
        let live = estimates.len();
        let n = T::from_usize(1 + live).unwrap();

        let mut peer_sum = T::zero();
        for e in &estimates {
            peer_sum = peer_sum + e.mean;
        }
        let cluster_estimate = input.local_rate + peer_sum;

        // Cluster-level PID, per-node share of the correction
        self.pid.set_setpoint(cluster_target);
        let correction = self.pid.compute_correction(cluster_estimate);

        let share = cluster_target / n;
        self.last_setpoint = Some(share);
        num_traits::clamp(
            share + correction / n,
            input.min_rate / n,
            input.max_rate / n,
        )
    }

    fn setpoint(&self) -> T {
        self.last_setpoint.unwrap_or_else(T::zero)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::PeerRate;
    use crate::pid_controller::PIDControllerBuilder;
    use std::time::Duration;

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
    fn test_at_target_no_correction() {
        let pid = PIDControllerBuilder::new(0.0).kp(0.5).build();
        let mut engine = HybridEngine::new(pid, HybridParams::default());
        // Cluster estimate = 100 local + 200 peers = 300 = target
        let peers = vec![peer("a", 100.0, 0), peer("b", 100.0, 0)];
        let refill = engine.update(&input(100.0, &peers, Some(300.0)));
        assert_eq!(refill, 100.0, "zero error → share");
        assert_eq!(engine.setpoint(), 100.0);
    }

    #[test]
    fn test_over_target_reduces_share() {
        let pid = PIDControllerBuilder::new(0.0).kp(0.5).build();
        let mut engine = HybridEngine::new(pid, HybridParams::default());
        // Estimate 450 vs target 300: correction = 0.5·(−150) = −75, /3 = −25
        let peers = vec![peer("a", 150.0, 0), peer("b", 150.0, 0)];
        let refill = engine.update(&input(150.0, &peers, Some(300.0)));
        assert_eq!(refill, 75.0);
    }

    #[test]
    fn test_correction_split_is_n_independent_in_aggregate() {
        // n nodes each applying correction/n must sum to one controller's
        // output regardless of n
        for n in [2usize, 5, 10] {
            let pid = PIDControllerBuilder::new(0.0).kp(1.0).build();
            let mut engine = HybridEngine::new(pid, HybridParams::default());
            let peers: Vec<_> = (0..n - 1)
                .map(|i| peer(&format!("p{}", i), 200.0 / n as f64, 0))
                .collect();
            let local = 200.0 / n as f64;
            let refill = engine.update(&input(local, &peers, Some(300.0)));
            let share = 300.0 / n as f64;
            let aggregate_correction = (refill - share) * n as f64;
            // Estimate = 200 total, error = +100, kp = 1 → aggregate +100
            assert!(
                (aggregate_correction - 100.0).abs() < 1e-6,
                "n={}: aggregate correction {}",
                n,
                aggregate_correction
            );
        }
    }

    #[test]
    fn test_single_node_mode_matches_pid_semantics() {
        let pid = PIDControllerBuilder::new(100.0).kp(0.5).build();
        let mut engine = HybridEngine::new(pid, HybridParams::default());
        let refill = engine.update(&input(80.0, &[], None));
        // error 20 → correction 10 → 110
        assert_eq!(refill, 110.0);
    }
}
