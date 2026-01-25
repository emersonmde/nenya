/// Scenario runner for deterministic time-based testing
use nenya::RateLimiter;
use std::time::{Duration, Instant};

use super::{MetricsCollector, RequestGenerator};

/// Results from running a single phase of a scenario
#[derive(Debug)]
// Note: Some fields unused but part of complete phase metrics API for diagnostic purposes
#[allow(dead_code)]
pub struct PhaseMetrics {
    pub duration_secs: f64,
    pub total_requests: usize,
    pub accepted_requests: usize,
    pub throttled_requests: usize,
    pub metrics: MetricsCollector,
}

/// Simulated time loop for running scenario tests
///
/// This runner advances time in small, deterministic steps and generates
/// requests according to a load pattern. Uses controlled timestamps via
/// `should_throttle_at()` for fast, deterministic testing without actual sleeping.
///
/// Request timestamps are spread evenly within each time step to simulate
/// realistic request arrival patterns (not bursty).
pub struct SimulatedTimeLoop {
    /// Time step duration (e.g., 10ms)
    pub dt: Duration,
}

impl SimulatedTimeLoop {
    /// Create a new simulated time loop with the given time step
    pub fn new(dt: Duration) -> Self {
        Self { dt }
    }

    /// Create a time loop with 10ms time steps (100 ticks per second)
    pub fn with_10ms_steps() -> Self {
        Self::new(Duration::from_millis(10))
    }

    /// Run a single phase of a scenario
    ///
    /// # Arguments
    /// * `duration_secs` - How long to run this phase (in seconds)
    /// * `load_generator` - Generator for request rate pattern
    /// * `rate_limiter` - The rate limiter to test
    /// * `base_time` - Optional base timestamp for the simulation. If None, uses current time.
    ///
    /// # Returns
    /// Phase metrics including all collected data
    pub fn run_phase<G: RequestGenerator>(
        &self,
        duration_secs: f64,
        load_generator: &G,
        rate_limiter: &mut RateLimiter<f64>,
    ) -> PhaseMetrics {
        self.run_phase_with_base_time(duration_secs, load_generator, rate_limiter, None)
    }

    /// Run a single phase with a specific base timestamp
    ///
    /// This allows multiple phases to share the same time base for continuity.
    pub fn run_phase_with_base_time<G: RequestGenerator>(
        &self,
        duration_secs: f64,
        load_generator: &G,
        rate_limiter: &mut RateLimiter<f64>,
        base_time: Option<Instant>,
    ) -> PhaseMetrics {
        let mut metrics = MetricsCollector::new();
        let mut current_time = 0.0;
        let dt_secs = self.dt.as_secs_f64();

        let mut total_requests = 0;
        let mut accepted_requests = 0;
        let mut throttled_requests = 0;

        // Use provided base time or current time
        let base_time = base_time.unwrap_or_else(Instant::now);

        // Fractional request accumulator to maintain accurate average rate
        // Without this, rounding causes rate errors (e.g., 1.5 always rounds to 2)
        let mut request_debt = 0.0;

        while current_time < duration_secs {
            // Generate requests for this tick
            let target_tps = load_generator.tps_at_time(current_time);
            request_debt += target_tps * dt_secs;
            let requests_this_tick = request_debt.floor() as usize;
            request_debt -= requests_this_tick as f64;

            let mut tick_throttled = 0;

            // If no requests this tick, still update state to keep windows trimmed
            // Use update_state_at() instead of should_throttle_at() to avoid adding fake timestamps
            if requests_this_tick == 0 {
                let tick_timestamp =
                    base_time + Duration::from_secs_f64(current_time + dt_secs / 2.0);
                rate_limiter.update_state_at(tick_timestamp);
            }

            // Spread requests evenly within this tick to simulate realistic arrival
            for i in 0..requests_this_tick {
                // Calculate timestamp for this request within the tick
                let offset_within_tick = if requests_this_tick > 1 {
                    (i as f64 / requests_this_tick as f64) * dt_secs
                } else {
                    dt_secs / 2.0 // Single request goes in middle of tick
                };

                let request_timestamp =
                    base_time + Duration::from_secs_f64(current_time + offset_within_tick);

                // Use time-controlled throttling
                let throttled = rate_limiter.should_throttle_at(request_timestamp);
                total_requests += 1;
                if throttled {
                    tick_throttled += 1;
                    throttled_requests += 1;
                } else {
                    accepted_requests += 1;
                }
            }

            // Calculate rates for this tick
            let tick_throttled_tps = tick_throttled as f64 / dt_secs;

            // Record metrics at end of tick
            // NOTE: We record the rate limiter's LOCAL accepted rate (not combined with external)
            // as this represents what we're actually accepting at this node
            let limiter_accepted_rate = rate_limiter.local_accepted_request_rate();

            metrics.record_tick(
                current_time,
                target_tps,
                limiter_accepted_rate, // Use LOCAL accepted rate
                tick_throttled_tps,
                rate_limiter.target_rate(),
                0.0, // PID error (not directly accessible via public API)
                0.0, // PID correction (not directly accessible via public API)
            );

            // Advance time (no actual sleeping - pure simulation)
            current_time += dt_secs;
        }

        PhaseMetrics {
            duration_secs,
            total_requests,
            accepted_requests,
            throttled_requests,
            metrics,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::ConstantLoadGenerator;
    use nenya::pid_controller::PIDControllerBuilder;
    use nenya::RateLimiterBuilder;

    #[test]
    fn test_simulated_time_loop_basic() {
        let pid = PIDControllerBuilder::new(100.0)
            .kp(0.8)
            .ki(0.05)
            .kd(0.04)
            .build();

        let mut rate_limiter = RateLimiterBuilder::new(100.0)
            .min_rate(50.0)
            .max_rate(150.0)
            .pid_controller(pid)
            .update_interval(Duration::from_millis(100))
            .build();

        let loop_runner = SimulatedTimeLoop::with_10ms_steps();
        let load_gen = ConstantLoadGenerator::new(80.0);

        let phase = loop_runner.run_phase(0.5, &load_gen, &mut rate_limiter);

        // Should have generated some requests
        assert!(phase.total_requests > 0);
        // Should have accepted most at 80 TPS (under 100 target)
        assert!(phase.accepted_requests > 0);
        // Should have collected metrics
        assert!(!phase.metrics.is_empty());
    }
}
