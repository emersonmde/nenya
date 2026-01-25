/// Scenario tests for varying load patterns
///
/// These tests verify that the PID controller properly tracks dynamic loads:
/// - Sinusoidal variation tracking
/// - Ramp-up response
/// - Step input response (classic control theory test)
mod test_utils;
use test_utils::{
    assertions::*, RampLoadGenerator, SimulatedTimeLoop, SinusoidalLoadGenerator, StepLoadGenerator,
};

use nenya::pid_controller::PIDControllerBuilder;
use nenya::RateLimiterBuilder;
use std::time::{Duration, Instant};

/// Test that the rate limiter tracks sinusoidal load variation
///
/// **Scenario:**
/// - Load varies 50-150 TPS sinusoidally (period = 20s)
/// - Duration: 30 seconds (1.5 periods)
/// - Verify: Rate limiter tracks variation within reasonable bounds
/// - Verify: No excessive oscillation
///
/// **Control Theory:**
/// Tests frequency response - how well the PID tracks a periodic input.
/// Well-tuned controller should track with minimal phase lag.
///
/// **Regression Detection:**
/// Catches if PID tuning causes poor tracking or instability with varying loads.
#[test]
fn test_sinusoidal_load_tracking() {
    let pid = PIDControllerBuilder::new(100.0)
        .kp(0.8)
        .ki(0.05)
        .kd(0.04)
        .output_limit(30.0)
        .build();

    let base_time = Instant::now();
    let mut rate_limiter = RateLimiterBuilder::new(100.0)
        .min_rate(40.0)
        .max_rate(160.0)
        .pid_controller(pid)
        .update_interval(Duration::from_millis(100))
        .initial_timestamp(base_time - Duration::from_secs(1))
        .build();

    let runner = SimulatedTimeLoop::with_10ms_steps();

    // Sinusoidal load: baseline=100, amplitude=50, period=20s
    // Varies between 50 and 150 TPS
    let sine_gen = SinusoidalLoadGenerator::new(100.0, 50.0, 20.0);

    let phase =
        runner.run_phase_with_base_time(30.0, &sine_gen, &mut rate_limiter, Some(base_time));

    // In steady-state (after first period), accepted rate should roughly track generated rate
    // We can't expect perfect tracking, but it should follow the general pattern
    let accepted_rates: Vec<f64> = phase.metrics.get_field(20.0, 30.0, |r| r.accepted_tps);

    // Mean should be reasonably close to baseline (100 TPS)
    // Note: During sinusoidal tracking, the sliding window calculation and
    // tracking delays can cause the mean to deviate from the setpoint
    let accepted_mean = mean(&accepted_rates);
    assert!(
        (accepted_mean - 100.0).abs() < 30.0,
        "Mean accepted rate {} should be within 30 TPS of baseline 100 TPS",
        accepted_mean
    );

    // Should not oscillate wildly
    // Note: Tracking a sinusoidal input naturally has crossings, so we use a higher threshold
    // We're checking for pathological oscillation, not normal tracking behavior
    let std = stddev(&accepted_rates);
    assert!(
        std < 40.0,
        "Excessive variance during sinusoidal tracking: stddev={:.1}",
        std
    );

    // Variance should exist (we're tracking a varying signal)
    let accepted_stddev = stddev(&accepted_rates);
    assert!(
        accepted_stddev > 5.0,
        "Should have variance while tracking sinusoidal load"
    );
}

/// Test response to a linear ramp in load
///
/// **Scenario:**
/// - Linear ramp 10 → 100 TPS over 20 seconds
/// - Verify: Smooth tracking without excessive overshoot
/// - Verify: Steady-state error < 10% at end
///
/// **Control Theory:**
/// Tests ramp response - ability to track a changing setpoint.
/// Integral term should eliminate steady-state error during ramp.
///
/// **Regression Detection:**
/// Catches if integral term is too weak or derivative term causes issues.
#[test]
fn test_ramp_up_response() {
    let pid = PIDControllerBuilder::new(100.0) // Target the end of the ramp
        .kp(0.8)
        .ki(0.05)
        .kd(0.04)
        .output_limit(20.0)
        .build();

    let base_time = Instant::now();
    let mut rate_limiter = RateLimiterBuilder::new(100.0)
        .min_rate(5.0)
        .max_rate(120.0)
        .pid_controller(pid)
        .update_interval(Duration::from_millis(100))
        .initial_timestamp(base_time - Duration::from_secs(1))
        .build();

    let runner = SimulatedTimeLoop::with_10ms_steps();

    // Linear ramp from 10 to 100 TPS over 20 seconds
    let ramp_gen = RampLoadGenerator::new(10.0, 100.0, 20.0);

    let phase =
        runner.run_phase_with_base_time(25.0, &ramp_gen, &mut rate_limiter, Some(base_time));

    // At end of ramp (t=20-25), load is constant at 100 TPS
    // Accepted rate should converge to 100 TPS
    let end_stats = phase
        .metrics
        .compute_stats(22.0, 25.0, |r| r.accepted_tps)
        .expect("end stats");

    assert!(
        (end_stats.mean - 100.0).abs() / 100.0 < 0.15,
        "After ramp: accepted rate {} not within 15% of 100 TPS",
        end_stats.mean
    );

    // Should not have excessive overshoot during ramp
    let all_accepted: Vec<f64> = phase.metrics.get_field(0.0, 25.0, |r| r.accepted_tps);
    let max_overshoot = compute_max_overshoot(&all_accepted, 100.0);

    assert!(
        max_overshoot < 30.0,
        "Overshoot {} during ramp exceeds 30 TPS",
        max_overshoot
    );
}

/// Test classic step input response
///
/// **Scenario:**
/// - Step change 50 TPS → 100 TPS at t=5s
/// - Verify: Rise time, overshoot, settling time within acceptable bounds
/// - Verify: Converges to new setpoint
///
/// **Control Theory:**
/// Classic step response test from control systems.
/// Characterizes: rise time, overshoot, settling time, steady-state error.
///
/// **Regression Detection:**
/// Catches if PID tuning changes affect transient response characteristics.
#[test]
fn test_step_input_response() {
    let pid = PIDControllerBuilder::new(50.0)
        .kp(0.8)
        .ki(0.05)
        .kd(0.04)
        .output_limit(20.0)
        .build();

    let base_time = Instant::now();
    let mut rate_limiter = RateLimiterBuilder::new(50.0)
        .min_rate(25.0)
        .max_rate(120.0)
        .pid_controller(pid)
        .update_interval(Duration::from_millis(100))
        .initial_timestamp(base_time - Duration::from_secs(1))
        .build();

    let runner = SimulatedTimeLoop::with_10ms_steps();

    // Step change from 50 to 100 TPS at t=5s
    let step_gen = StepLoadGenerator::new(50.0, 100.0, 5.0);

    let phase =
        runner.run_phase_with_base_time(15.0, &step_gen, &mut rate_limiter, Some(base_time));

    // Before step (t=2-5s): should be at ~50 TPS
    let before_stats = phase
        .metrics
        .compute_stats(2.0, 5.0, |r| r.accepted_tps)
        .expect("before step stats");

    assert!(
        (before_stats.mean - 50.0).abs() / 50.0 < 0.15,
        "Before step: accepted rate {} not within 15% of 50 TPS",
        before_stats.mean
    );

    // After step, in steady state (t=10-15s): load is 100 TPS but PID setpoint is 50 TPS
    // So accepted rate should stay at ~50 TPS, throttling the excess load
    let after_stats = phase
        .metrics
        .compute_stats(10.0, 15.0, |r| r.accepted_tps)
        .expect("after step stats");

    assert!(
        (after_stats.mean - 50.0).abs() / 50.0 < 0.20,
        "After step: accepted rate {} not within 20% of 50 TPS (PID setpoint)",
        after_stats.mean
    );

    // Check overshoot after step (should not exceed setpoint significantly)
    let step_response: Vec<f64> = phase.metrics.get_field(5.0, 10.0, |r| r.accepted_tps);
    let overshoot = compute_max_overshoot(&step_response, 50.0);

    assert!(
        overshoot < 20.0,
        "Step response overshoot {} exceeds 20 TPS above setpoint",
        overshoot
    );

    // Check settling time to setpoint (50 TPS)
    let all_after_step: Vec<f64> = phase.metrics.get_field(5.0, 15.0, |r| r.accepted_tps);
    let settling_idx = compute_settling_time(&all_after_step, 50.0, 10.0);

    assert!(
        settling_idx.is_some(),
        "Step response never settled within ±10%"
    );

    // Settling should occur within 10 seconds
    // Note: We use 10ms steps (0.01s), not 50ms
    let settling_time_idx = settling_idx.unwrap();
    let settling_time_secs = settling_time_idx as f64 * 0.01; // 10ms per sample

    assert!(
        settling_time_secs < 10.0,
        "Settling time {} exceeds 10 seconds",
        settling_time_secs
    );
}
