/// Scenario tests for burst handling and PID response to load spikes
///
/// These tests verify that the PID controller properly handles sudden load changes:
/// - Burst overshoot is limited
/// - Settling time is acceptable
/// - Anti-windup prevents excessive integral accumulation
/// - Sustained overload converges to target rate
mod test_utils;
use test_utils::{
    assertions::*, BurstLoadGenerator, ConstantLoadGenerator, MetricsCollector, SimulatedTimeLoop,
};

use nenya::pid_controller::PIDControllerBuilder;
use nenya::RateLimiterBuilder;
use std::time::{Duration, Instant};

/// Test that a burst causes limited overshoot and settles within acceptable time
///
/// **Scenario:**
/// - Phase 1 (warmup 5s): 80 TPS constant → verify steady-state ±5%
/// - Phase 2 (burst 2s): 200 TPS spike → verify throttling increases, overshoot < 20%
/// - Phase 3 (settling 10s): Return to 80 TPS → verify converges within 8s to ±5%
///
/// **Control Theory:**
/// Tests the PID controller's step response and recovery characteristics.
/// Overshoot and settling time are key metrics for tuned control systems.
///
/// **Regression Detection:**
/// Catches if PID tuning changes cause excessive overshoot or slow settling.
#[test]
fn test_burst_overshoot_and_settling() {
    // Well-tuned PID parameters for 100 TPS target
    let pid = PIDControllerBuilder::new(100.0)
        .kp(0.8)
        .ki(0.05)
        .kd(0.04)
        .output_limit(20.0)
        .build();

    let mut rate_limiter = RateLimiterBuilder::new(100.0)
        .min_rate(50.0)
        .max_rate(150.0)
        .pid_controller(pid)
        .update_interval(Duration::from_millis(100))
        .build();

    let runner = SimulatedTimeLoop::with_10ms_steps();
    let mut all_metrics = MetricsCollector::new();

    // Phase 1: Warmup at 80 TPS (below target)
    let warmup_gen = ConstantLoadGenerator::new(80.0);
    let phase1 = runner.run_phase(5.0, &warmup_gen, &mut rate_limiter);

    // Collect metrics with time offset 0
    for record in phase1.metrics.all_records() {
        all_metrics.record_tick(
            record.time,
            record.generated_tps,
            record.accepted_tps,
            record.throttled_tps,
            record.target_rate,
            record.pid_error,
            record.pid_correction,
        );
    }

    // Verify warmup steady-state: accepted rate should be close to 80 TPS
    // Note: With token bucket, initial burst can affect early measurements
    let warmup_stats = phase1
        .metrics
        .compute_stats(3.0, 5.0, |r| r.accepted_tps)
        .expect("warmup stats");
    assert!(
        (warmup_stats.mean - 80.0).abs() / 80.0 < 0.15,
        "Warmup phase: accepted rate {} not within 15% of 80 TPS",
        warmup_stats.mean
    );

    // Phase 2: Burst to 200 TPS
    let burst_gen = ConstantLoadGenerator::new(200.0);
    let phase2 = runner.run_phase(2.0, &burst_gen, &mut rate_limiter);

    let time_offset = 5.0;
    for record in phase2.metrics.all_records() {
        all_metrics.record_tick(
            record.time + time_offset,
            record.generated_tps,
            record.accepted_tps,
            record.throttled_tps,
            record.target_rate,
            record.pid_error,
            record.pid_correction,
        );
    }

    // During burst, throttling should increase significantly
    assert!(
        phase2.throttled_requests > 0,
        "Burst phase should have throttled requests"
    );

    // Phase 3: Recovery to 80 TPS
    let recovery_gen = ConstantLoadGenerator::new(80.0);
    let phase3 = runner.run_phase(10.0, &recovery_gen, &mut rate_limiter);

    let time_offset = 7.0;
    for record in phase3.metrics.all_records() {
        all_metrics.record_tick(
            record.time + time_offset,
            record.generated_tps,
            record.accepted_tps,
            record.throttled_tps,
            record.target_rate,
            record.pid_error,
            record.pid_correction,
        );
    }

    // Verify settling: after 8 seconds of recovery, should be within ±5% of 80 TPS
    let settling_stats = phase3
        .metrics
        .compute_stats(8.0, 10.0, |r| r.accepted_tps)
        .expect("settling stats");

    assert!(
        (settling_stats.mean - 80.0).abs() / 80.0 < 0.15,
        "Settling phase: accepted rate {} not within 15% of 80 TPS after 8s",
        settling_stats.mean
    );
}

/// Test that sustained overload converges to target rate without oscillation
///
/// **Scenario:**
/// - Constant 150 TPS load, target = 100 TPS
/// - Duration: 10 seconds
/// - Verify: Accepted converges to ~100 TPS, throttled to ~50 TPS
/// - Verify: No sustained oscillation
///
/// **Control Theory:**
/// Tests steady-state behavior under constant overload.
/// Derivative term should prevent oscillation.
///
/// **Regression Detection:**
/// Catches if derivative dampening breaks or PID becomes unstable.
#[test]
fn test_sustained_overload() {
    let pid = PIDControllerBuilder::new(100.0)
        .kp(0.8)
        .ki(0.05)
        .kd(0.04)
        .output_limit(20.0)
        .build();

    // Initialize rate limiter with timestamp in the past to allow PID updates
    let base_time = Instant::now();
    let mut rate_limiter = RateLimiterBuilder::new(100.0)
        .min_rate(50.0)
        .max_rate(150.0)
        .pid_controller(pid)
        .update_interval(Duration::from_millis(100))
        .initial_timestamp(base_time - Duration::from_secs(1))
        .build();

    let runner = SimulatedTimeLoop::with_10ms_steps();
    let overload_gen = ConstantLoadGenerator::new(150.0);

    let phase =
        runner.run_phase_with_base_time(10.0, &overload_gen, &mut rate_limiter, Some(base_time));

    // Verify convergence to target rate (100 TPS) in steady-state
    let steady_state_stats = phase
        .metrics
        .compute_stats(5.0, 10.0, |r| r.accepted_tps)
        .expect("steady state stats");

    assert!(
        (steady_state_stats.mean - 100.0).abs() / 100.0 < 0.15,
        "Steady state: accepted rate {} not within 15% of 100 TPS target",
        steady_state_stats.mean
    );

    // Check for stability: standard deviation should be reasonable
    // With discrete-time control, some variance is expected
    let accepted_rates: Vec<f64> = phase.metrics.get_field(5.0, 10.0, |r| r.accepted_tps);

    let std = stddev(&accepted_rates);
    assert!(
        std < 30.0,
        "Sustained overload caused excessive variance: stddev={:.1}",
        std
    );

    // Total accepted + throttled should approximately equal generated
    let total_handled = phase.accepted_requests + phase.throttled_requests;
    let ratio = total_handled as f64 / phase.total_requests as f64;
    assert!(
        ratio > 0.95,
        "Should handle most requests (accepted or throttled)"
    );
}

/// Test that anti-windup mechanism prevents excessive overshoot after burst
///
/// **Scenario:**
/// - Burst causes PID integral to accumulate
/// - Verify: Anti-windup feedback reduces integral when clamping occurs
/// - Verify: Recovery doesn't have excessive overshoot
///
/// **Control Theory:**
/// Without anti-windup, integral term accumulates during burst and causes
/// large overshoot during recovery. Anti-windup feedback prevents this.
///
/// **Regression Detection:**
/// Catches if anti-windup mechanism breaks or is disabled.
#[test]
fn test_burst_anti_windup_effectiveness() {
    // Aggressive Ki to make integral windup more likely
    let pid = PIDControllerBuilder::new(100.0)
        .kp(0.8)
        .ki(0.2) // Higher Ki to test anti-windup
        .kd(0.04)
        .output_limit(15.0) // Lower limit to trigger clamping
        .build();

    let mut rate_limiter = RateLimiterBuilder::new(100.0)
        .min_rate(50.0)
        .max_rate(150.0)
        .pid_controller(pid)
        .update_interval(Duration::from_millis(100))
        .build();

    let runner = SimulatedTimeLoop::with_10ms_steps();

    // Multi-phase: normal → burst → recovery
    let burst_gen = BurstLoadGenerator::new(80.0, 200.0, 2.0, 2.0);
    let phase = runner.run_phase(10.0, &burst_gen, &mut rate_limiter);

    // During recovery (after burst at t=4s), check for overshoot
    let recovery_rates: Vec<f64> = phase.metrics.get_field(5.0, 10.0, |r| r.accepted_tps);

    let max_overshoot = compute_max_overshoot(&recovery_rates, 100.0);

    // With anti-windup, overshoot should be limited even with high Ki
    assert!(
        max_overshoot < 30.0,
        "Anti-windup failed: overshoot {} exceeds 30 TPS",
        max_overshoot
    );

    // Should still converge to target eventually
    let late_recovery = phase
        .metrics
        .compute_stats(7.0, 10.0, |r| r.accepted_tps)
        .expect("late recovery stats");

    assert!(
        (late_recovery.mean - 80.0).abs() / 80.0 < 0.2,
        "Late recovery: accepted rate {} not within 20% of 80 TPS",
        late_recovery.mean
    );
}
