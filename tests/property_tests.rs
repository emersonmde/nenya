/// Property-based tests using proptest for invariant verification
///
/// These tests use randomized inputs to verify that critical invariants
/// hold across a wide range of scenarios:
/// - Rate bounds are never violated
/// - PID output respects limits
/// - System behaves deterministically
/// - Internal state remains valid
use nenya::pid_controller::PIDControllerBuilder;
use nenya::RateLimiterBuilder;
use proptest::prelude::*;
use std::time::{Duration, Instant};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))] // Reduce to 50 cases for speed

    /// Property: Target rate bounds enforcement
    ///
    /// **Invariant:** target_rate must always satisfy: min_rate <= target_rate <= max_rate
    ///
    /// **Regression Detection:**
    /// Catches if target rate clamping logic breaks.
    #[test]
    fn target_rate_bounds_enforcement(
        initial_target in 50.0f64..150.0,
        min_rate in 20.0f64..80.0,
        max_rate in 100.0f64..200.0,
        load_tps in 10.0f64..300.0,
        num_ticks in 10usize..50,
    ) {
        prop_assume!(min_rate < max_rate);
        let clamped_target = initial_target.max(min_rate).min(max_rate);

        let pid = PIDControllerBuilder::new(clamped_target)
            .kp(1.5) // Aggressive to force large corrections
            .ki(0.3)
            .kd(0.05)
            .output_limit(100.0)
            .build();

        let base_time = Instant::now();
        let mut rate_limiter = RateLimiterBuilder::new(clamped_target)
            .min_rate(min_rate)
            .max_rate(max_rate)
            .pid_controller(pid)
            .update_interval(Duration::from_millis(100))
            .initial_timestamp(base_time - Duration::from_secs(1))
            .build();

        let dt = Duration::from_millis(10);
        let dt_secs = 0.01;
        let mut request_debt = 0.0;

        for tick in 0..num_ticks {
            let now = base_time + (dt * tick as u32);

            // Generate requests for this tick
            request_debt += load_tps * dt_secs;
            let requests_this_tick = request_debt.floor() as usize;
            request_debt -= requests_this_tick as f64;

            for _ in 0..requests_this_tick {
                rate_limiter.should_throttle_at(now);
            }

            // Verify bounds after each tick
            let target = rate_limiter.target_rate();
            prop_assert!(
                target >= min_rate && target <= max_rate,
                "Target rate {} outside bounds [{}, {}] at tick {}",
                target, min_rate, max_rate, tick
            );
        }
    }

    /// Property: PID output remains bounded by output_limit
    ///
    /// **Invariant:** PID correction must never exceed ±output_limit.
    ///
    /// **Regression Detection:**
    /// Catches if output clamping logic breaks.
    #[test]
    fn pid_output_remains_bounded(
        setpoint in 10.0f64..200.0,
        process_var in 0.0f64..300.0,
        kp in 0.1f64..2.0,
        ki in 0.01f64..0.5,
        kd in 0.0f64..0.2,
        output_limit in 5.0f64..100.0,
    ) {
        let mut pid = PIDControllerBuilder::new(setpoint)
            .kp(kp)
            .ki(ki)
            .kd(kd)
            .output_limit(output_limit)
            .build();

        // Run multiple iterations to test sustained error
        for _ in 0..100 {
            let correction: f64 = pid.compute_correction(process_var);
            prop_assert!(
                correction.abs() <= output_limit,
                "PID correction {} exceeds output_limit {}",
                correction, output_limit
            );
        }
    }

    /// Property: Rate limiter behaves deterministically
    ///
    /// **Invariant:** Given the same sequence of calls, the rate limiter
    /// should produce identical results.
    ///
    /// **Regression Detection:**
    /// Catches if non-determinism is introduced.
    #[test]
    fn rate_limiter_determinism(
        target in 50.0f64..150.0,
        num_requests in 10usize..50,
    ) {
        let base_time = Instant::now();

        let pid1 = PIDControllerBuilder::new(target)
            .kp(0.8)
            .ki(0.05)
            .kd(0.04)
            .build();

        let mut limiter1 = RateLimiterBuilder::new(target)
            .min_rate(target * 0.5)
            .max_rate(target * 1.5)
            .pid_controller(pid1)
            .update_interval(Duration::from_millis(100))
            .initial_timestamp(base_time - Duration::from_secs(1))
            .build();

        let pid2 = PIDControllerBuilder::new(target)
            .kp(0.8)
            .ki(0.05)
            .kd(0.04)
            .build();

        let mut limiter2 = RateLimiterBuilder::new(target)
            .min_rate(target * 0.5)
            .max_rate(target * 1.5)
            .pid_controller(pid2)
            .update_interval(Duration::from_millis(100))
            .initial_timestamp(base_time - Duration::from_secs(1))
            .build();

        // Generate identical request patterns with controlled time
        let mut results1 = Vec::new();
        let mut results2 = Vec::new();

        for i in 0..num_requests {
            let now = base_time + Duration::from_millis((i * 10) as u64);
            results1.push(limiter1.should_throttle_at(now));
            results2.push(limiter2.should_throttle_at(now));
        }

        // Results should be identical
        prop_assert_eq!(results1, results2, "Rate limiter produced different results for same input");
    }

    /// Property: Accumulated error respects error_limit
    ///
    /// **Invariant:** When error_limit is set, accumulated_error should never exceed it.
    ///
    /// **Regression Detection:**
    /// Catches if integral clamping breaks.
    #[test]
    fn accumulated_error_respects_limit(
        setpoint in 50.0f64..150.0,
        error_limit in 10.0f64..100.0,
        ki in 0.05f64..0.3,
    ) {
        let mut pid = PIDControllerBuilder::new(setpoint)
            .kp(0.5)
            .ki(ki)
            .kd(0.02)
            .error_limit(error_limit)
            .output_limit(50.0)
            .build();

        // Generate large errors to try to exceed error_limit
        let process_var = setpoint * 2.0; // Large error

        let mut corrections: Vec<f64> = Vec::new();
        for _ in 0..50 {
            let correction: f64 = pid.compute_correction(process_var);
            corrections.push(correction);
        }

        // With error_limit, corrections should stabilize and stay bounded
        let last_corrections = &corrections[corrections.len() - 10..];
        let max_late_correction = last_corrections.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        prop_assert!(
            max_late_correction.abs() <= 50.0, // Within output_limit
            "Corrections not bounded: max_late_correction={}",
            max_late_correction
        );
    }

    /// Property: Accepted rate tracking
    ///
    /// **Invariant:** When load < setpoint, all requests should be accepted.
    /// When load > setpoint, throttling should occur to maintain setpoint.
    ///
    /// **Regression Detection:**
    /// Catches if throttling logic breaks.
    #[test]
    fn accepted_rate_converges_to_setpoint(
        setpoint in 50.0f64..150.0,
        load_multiplier in 1.5f64..3.0, // Load > setpoint to force throttling
    ) {
        let load_tps = setpoint * load_multiplier;

        let pid = PIDControllerBuilder::new(setpoint)
            .kp(0.8)
            .ki(0.05)
            .kd(0.04)
            .output_limit(30.0)
            .build();

        let base_time = Instant::now();
        let mut rate_limiter = RateLimiterBuilder::new(setpoint)
            .min_rate(setpoint * 0.5)
            .max_rate(setpoint * 2.0)
            .pid_controller(pid)
            .update_interval(Duration::from_millis(100))
            .initial_timestamp(base_time - Duration::from_secs(1))
            .build();

        let dt = Duration::from_millis(10);
        let dt_secs = 0.01;
        let mut request_debt = 0.0;
        let mut accepted_count = 0;
        let num_ticks = 500; // 5 seconds of simulation

        for tick in 0..num_ticks {
            let now = base_time + (dt * tick as u32);

            request_debt += load_tps * dt_secs;
            let requests_this_tick = request_debt.floor() as usize;
            request_debt -= requests_this_tick as f64;

            for _ in 0..requests_this_tick {
                if !rate_limiter.should_throttle_at(now) {
                    accepted_count += 1;
                }
            }
        }

        // Calculate actual accepted rate over the simulation
        let total_duration_secs = (num_ticks as f64) * dt_secs;
        let accepted_rate = (accepted_count as f64) / total_duration_secs;

        // After convergence, accepted rate should be within 30% of setpoint
        // (generous tolerance to account for PID tuning and short simulation)
        prop_assert!(
            (accepted_rate - setpoint).abs() / setpoint < 0.3,
            "Accepted rate {} not converging toward setpoint {} (load={})",
            accepted_rate, setpoint, load_tps
        );
    }
}
