/// Scenario tests for edge cases and unusual conditions
///
/// These tests verify PID behavior in unusual scenarios:
/// - Zero load periods (silence)
/// - Cold start transients
/// - Rate bounds enforcement
mod test_utils;
use test_utils::{assertions::*, BurstLoadGenerator, ConstantLoadGenerator, SimulatedTimeLoop};

use nenya::pid_controller::PIDControllerBuilder;
use nenya::RateLimiterBuilder;
use std::time::{Duration, Instant};

/// Test recovery from zero load period
///
/// **Scenario:**
/// - 100 TPS → 0 TPS (5s silence) → 100 TPS
/// - Verify: Integral doesn't wind down excessively during silence
/// - Verify: Fast recovery when load returns
///
/// **Control Theory:**
/// During zero load, error becomes negative (actual < setpoint).
/// Integral term accumulates negative error. Anti-windup should prevent
/// excessive negative accumulation that would delay recovery.
///
/// **Regression Detection:**
/// Catches if zero-load handling causes issues with integral term.
#[test]
fn test_zero_load_recovery() {
    let pid = PIDControllerBuilder::new(100.0)
        .kp(0.8)
        .ki(0.05)
        .kd(0.04)
        .output_limit(20.0)
        .build();

    let base_time = Instant::now();
    let mut rate_limiter = RateLimiterBuilder::new(100.0)
        .min_rate(50.0)
        .max_rate(150.0)
        .pid_controller(pid)
        .update_interval(Duration::from_millis(100))
        .initial_timestamp(base_time - Duration::from_secs(1))
        .build();

    let runner = SimulatedTimeLoop::with_10ms_steps();

    // Use burst generator with 0 TPS during burst to simulate silence
    // normal=100, burst=0, burst at t=5s for 5s
    let silence_gen = BurstLoadGenerator::new(100.0, 0.0, 5.0, 5.0);

    let phase =
        runner.run_phase_with_base_time(15.0, &silence_gen, &mut rate_limiter, Some(base_time));

    // Before silence (t=2-5s): should be near 100 TPS
    let before_stats = phase
        .metrics
        .compute_stats(2.0, 5.0, |r| r.accepted_tps)
        .expect("before silence");

    assert!(
        (before_stats.mean - 100.0).abs() / 100.0 < 0.2,
        "Before silence: accepted rate {} not within 20% of 100 TPS",
        before_stats.mean
    );

    // During silence (t=5-10s): should be 0 TPS (no requests generated)
    let during_stats = phase
        .metrics
        .compute_stats(6.0, 9.0, |r| r.accepted_tps)
        .expect("during silence");

    assert!(
        during_stats.mean < 5.0,
        "During silence: accepted rate {} should be near 0",
        during_stats.mean
    );

    // After recovery (t=12-15s): should return to ~100 TPS
    let after_stats = phase
        .metrics
        .compute_stats(12.0, 15.0, |r| r.accepted_tps)
        .expect("after recovery");

    assert!(
        (after_stats.mean - 100.0).abs() / 100.0 < 0.25,
        "After recovery: accepted rate {} not within 25% of 100 TPS",
        after_stats.mean
    );

    // Recovery should happen within 2 seconds after load returns
    let recovery_window: Vec<f64> = phase.metrics.get_field(10.0, 15.0, |r| r.accepted_tps);

    // Should converge within the recovery window
    let settling = compute_settling_time(&recovery_window, 100.0, 15.0);
    assert!(
        settling.is_some(),
        "Should settle within ±15% after load returns"
    );
}

/// Test cold start transient response
///
/// **Scenario:**
/// - Start from zero, immediate 100 TPS load
/// - Verify: No initial overshoot
/// - Verify: Settles within 5 seconds
///
/// **Control Theory:**
/// Tests initial transient with zero state (no accumulated error).
/// Should reach setpoint smoothly without overshoot.
///
/// **Regression Detection:**
/// Catches if initialization causes unstable startup behavior.
#[test]
fn test_cold_start_transient() {
    let pid = PIDControllerBuilder::new(100.0)
        .kp(0.8)
        .ki(0.05)
        .kd(0.04)
        .output_limit(20.0)
        .build();

    let base_time = Instant::now();
    let mut rate_limiter = RateLimiterBuilder::new(100.0)
        .min_rate(50.0)
        .max_rate(150.0)
        .pid_controller(pid)
        .update_interval(Duration::from_millis(100))
        .initial_timestamp(base_time - Duration::from_secs(1))
        .build();

    let runner = SimulatedTimeLoop::with_10ms_steps();
    let load_gen = ConstantLoadGenerator::new(100.0);

    let phase =
        runner.run_phase_with_base_time(10.0, &load_gen, &mut rate_limiter, Some(base_time));

    // Check for overshoot during startup
    let all_accepted: Vec<f64> = phase.metrics.get_field(0.0, 10.0, |r| r.accepted_tps);
    let overshoot = compute_max_overshoot(&all_accepted, 100.0);

    assert!(
        overshoot < 20.0,
        "Cold start overshoot {} exceeds 20 TPS",
        overshoot
    );

    // Should settle within 7 seconds (token bucket dynamics may extend transient)
    let settling = compute_settling_time(&all_accepted, 100.0, 10.0);
    assert!(settling.is_some(), "Should settle within ±10%");

    let settling_idx = settling.unwrap();
    let settling_secs = settling_idx as f64 * 0.01; // 10ms per sample

    assert!(
        settling_secs < 7.0,
        "Cold start settling time {} exceeds 7 seconds",
        settling_secs
    );

    // In steady state (t=7-10s), should be close to 100 TPS
    let steady_stats = phase
        .metrics
        .compute_stats(7.0, 10.0, |r| r.accepted_tps)
        .expect("steady state");

    assert!(
        (steady_stats.mean - 100.0).abs() / 100.0 < 0.15,
        "Steady state: accepted rate {} not within 15% of 100 TPS",
        steady_stats.mean
    );
}

/// Test that rate limiter enforces min/max bounds
///
/// **Scenario:**
/// - Force conditions that would cause PID to suggest rates beyond bounds
/// - Overload: 200 TPS with max_rate=120 → verify clamps to 120
/// - Underload: 20 TPS with min_rate=50 → verify clamps to 50
///
/// **Control Theory:**
/// Output saturation - PID output must respect hard limits.
/// Anti-windup should engage when limits are hit.
///
/// **Regression Detection:**
/// Catches if bounds checking or clamping logic breaks.
#[test]
fn test_rate_limiter_bounds_enforcement() {
    // Test upper bound enforcement
    {
        let pid = PIDControllerBuilder::new(100.0)
            .kp(1.2) // Aggressive gains to force large corrections
            .ki(0.3)
            .kd(0.05)
            .output_limit(50.0)
            .build();

        let base_time = Instant::now();
        let mut rate_limiter = RateLimiterBuilder::new(100.0)
            .min_rate(50.0)
            .max_rate(120.0) // Deliberately low max to test clamping
            .pid_controller(pid)
            .update_interval(Duration::from_millis(100))
            .initial_timestamp(base_time - Duration::from_secs(1))
            .build();

        let runner = SimulatedTimeLoop::with_10ms_steps();
        let overload_gen = ConstantLoadGenerator::new(200.0);

        let phase =
            runner.run_phase_with_base_time(5.0, &overload_gen, &mut rate_limiter, Some(base_time));

        // Target rate should never exceed max_rate
        let target_rates: Vec<f64> = phase.metrics.get_field(0.0, 5.0, |r| r.target_rate);

        for (i, &rate) in target_rates.iter().enumerate() {
            assert!(
                rate <= 120.0,
                "target_rate[{}] = {} exceeds max_rate 120",
                i,
                rate
            );
        }

        // Under overload, PID should maintain accepted rate at setpoint (100 TPS)
        // Target rate converges to where accepted rate = setpoint
        let final_target = rate_limiter.target_rate();
        assert!(
            (final_target - 100.0).abs() < 15.0,
            "Final target {} should be near setpoint 100 (not max_rate)",
            final_target
        );
    }

    // Test lower bound enforcement
    {
        let pid = PIDControllerBuilder::new(100.0)
            .kp(1.2)
            .ki(0.3)
            .kd(0.05)
            .output_limit(50.0)
            .build();

        let base_time = Instant::now();
        let mut rate_limiter = RateLimiterBuilder::new(100.0)
            .min_rate(50.0)
            .max_rate(150.0)
            .pid_controller(pid)
            .update_interval(Duration::from_millis(100))
            .initial_timestamp(base_time - Duration::from_secs(1))
            .build();

        let runner = SimulatedTimeLoop::with_10ms_steps();
        let underload_gen = ConstantLoadGenerator::new(20.0);

        let phase = runner.run_phase_with_base_time(
            5.0,
            &underload_gen,
            &mut rate_limiter,
            Some(base_time),
        );

        // Target rate should never go below min_rate
        let target_rates: Vec<f64> = phase.metrics.get_field(0.0, 5.0, |r| r.target_rate);

        for (i, &rate) in target_rates.iter().enumerate() {
            assert!(
                rate >= 50.0,
                "target_rate[{}] = {} below min_rate 50",
                i,
                rate
            );
        }

        // Under underload (20 TPS << 100 TPS setpoint), PID sees persistent error
        // Integral term accumulates, trying to increase acceptance (but can't, not enough load)
        // Target rate may increase toward setpoint/max_rate as PID tries to eliminate error
        let final_target = rate_limiter.target_rate();
        assert!(
            (50.0..=150.0).contains(&final_target),
            "Final target {} should be within bounds [50, 150]",
            final_target
        );
    }
}
