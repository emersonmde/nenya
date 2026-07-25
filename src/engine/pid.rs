//! The equal-division PID engine — the original Nenya control loop ported
//! behind [`RateController`] with zero behavior change (verified against the
//! Milestone 4 scenario-matrix baseline at seed 42).

use std::time::Duration;

use num_traits::{Float, FromPrimitive, Signed};

use super::{
    staleness_weight, ControlInput, PeerRate, RateController, DEFAULT_STALE_TIMEOUT,
    DEFAULT_SYNC_INTERVAL,
};
use crate::pid_controller::PIDController;

/// Equal-division PID control.
///
/// In distributed mode each node targets `cluster_target / (1 + live_peers)`
/// and uses only its *local* accepted rate as the feedback signal; the peer
/// observations contribute liveness (via [`staleness_weight`]) but not a
/// feedback term. This makes the loop immune to gossip noise at the cost of
/// serving skewed demand poorly (equal shares regardless of where the load
/// lands).
///
/// The staleness parameters default to the production gossip defaults
/// (500 ms sync interval, 10 s stale timeout — see `Config::from_env`); set
/// them explicitly with [`PidEngine::with_staleness`] when the deployment
/// overrides those.
#[derive(Debug, Clone)]
pub struct PidEngine<T> {
    pid: PIDController<T>,
    sync_interval: Duration,
    stale_timeout: Duration,
}

impl<T: Float + Signed + FromPrimitive + Copy> PidEngine<T> {
    pub fn new(pid: PIDController<T>) -> Self {
        PidEngine {
            pid,
            sync_interval: DEFAULT_SYNC_INTERVAL,
            stale_timeout: DEFAULT_STALE_TIMEOUT,
        }
    }

    /// Override the staleness parameters used for the liveness test.
    pub fn with_staleness(mut self, sync_interval: Duration, stale_timeout: Duration) -> Self {
        self.sync_interval = sync_interval;
        self.stale_timeout = stale_timeout;
        self
    }

    /// Number of peers whose observations still carry weight.
    fn live_peers(&self, peers: &[PeerRate<T>]) -> usize {
        peers
            .iter()
            .filter(|p| staleness_weight(p.age, self.sync_interval, self.stale_timeout) > 0.0)
            .count()
    }
}

impl<T: Float + Signed + FromPrimitive + Copy + Send + Sync + std::fmt::Debug> RateController<T>
    for PidEngine<T>
{
    fn update(&mut self, input: &ControlInput<'_, T>) -> T {
        // Determine setpoint and bounds based on coordination mode
        let (setpoint, min_bound, max_bound) = if let Some(cluster_target) = input.cluster_target {
            // Distributed mode: each node tracks toward its equal share and
            // scales the configured bounds by the live node count
            let num_nodes = T::from_usize(1 + self.live_peers(input.peers)).unwrap();
            (
                cluster_target / num_nodes,
                input.min_rate / num_nodes,
                input.max_rate / num_nodes,
            )
        } else {
            // Single-node mode: track the local target
            (input.target_rate, input.min_rate, input.max_rate)
        };

        self.pid.set_setpoint(setpoint);
        let correction = self.pid.compute_correction(input.local_rate);

        num_traits::clamp(setpoint + correction, min_bound, max_bound)
    }

    fn setpoint(&self) -> T {
        self.pid.setpoint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pid_controller::PIDControllerBuilder;

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

    fn peer(id: &str, rate: f64, age: Duration) -> PeerRate<f64> {
        PeerRate {
            id: id.to_string(),
            rate,
            age,
        }
    }

    #[test]
    fn test_single_node_tracks_target() {
        let pid = PIDControllerBuilder::new(100.0).kp(0.5).build();
        let mut engine = PidEngine::new(pid);
        // Under target: refill rises above baseline
        let refill = engine.update(&input(80.0, &[], None));
        assert!(refill > 100.0);
        assert_eq!(engine.setpoint(), 100.0);
    }

    #[test]
    fn test_distributed_equal_division() {
        let pid = PIDControllerBuilder::new(0.0).kp(0.0).build(); // no correction
        let mut engine = PidEngine::new(pid);
        let peers = vec![
            peer("a", 100.0, Duration::ZERO),
            peer("b", 100.0, Duration::ZERO),
        ];
        // 3 live nodes → setpoint = 300/3, zero gains → refill = setpoint
        let refill = engine.update(&input(100.0, &peers, Some(300.0)));
        assert_eq!(refill, 100.0);
        assert_eq!(engine.setpoint(), 100.0);
    }

    #[test]
    fn test_stale_peers_dont_count() {
        let pid = PIDControllerBuilder::new(0.0).kp(0.0).build();
        let mut engine = PidEngine::new(pid);
        let peers = vec![
            peer("live", 100.0, Duration::ZERO),
            peer("dead", 100.0, Duration::from_secs(60)),
        ];
        // Only 2 live nodes → setpoint = 300/2
        let refill = engine.update(&input(100.0, &peers, Some(300.0)));
        assert_eq!(refill, 150.0);
    }

    #[test]
    fn test_distributed_bounds_scale_with_liveness() {
        // Large positive correction clamps at max_rate / n
        let pid = PIDControllerBuilder::new(0.0).kp(100.0).build();
        let mut engine = PidEngine::new(pid);
        let peers = vec![peer("a", 0.0, Duration::ZERO)];
        let refill = engine.update(&input(0.0, &peers, Some(300.0)));
        assert_eq!(refill, 300.0); // max_rate 600 / 2 nodes
    }
}
