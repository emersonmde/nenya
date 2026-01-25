//! Performance benchmarks for PID Controller
//!
//! These benchmarks measure the computational overhead of the PID control algorithm.
//! The PID controller is called periodically (e.g., every 100ms) to adjust the target rate,
//! so it's less performance-critical than the throttling decision path.
//!
//! However, PID computation still happens in the hot path (during `should_throttle()`),
//! so we want to ensure it's fast enough not to add significant latency.
//!
//! ## Key Metrics
//!
//! - **Computation time**: Time to compute a single correction (target: <100ns)
//! - **Complexity**: Verify O(1) behavior regardless of history
//! - **Numerical stability**: Ensure no degradation with extreme values
//!
//! ## Running Benchmarks
//!
//! ```bash
//! cargo bench --bench pid_controller_bench
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use nenya::pid_controller::PIDControllerBuilder;

/// Benchmark: Single PID computation
///
/// Measures the time to compute one PID correction given an error.
/// This is the core computational cost of the PID algorithm.
fn single_pid_computation(c: &mut Criterion) {
    let mut group = c.benchmark_group("pid_computation");

    // P-only controller (simplest case)
    group.bench_function("p_only", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.8)
                .ki(0.0)
                .kd(0.0)
                .build();

        b.iter(|| {
            let process_var = black_box(85.0f64);
            black_box(pid.compute_correction(process_var))
        })
    });

    // PI controller (common configuration)
    group.bench_function("pi", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.8)
                .ki(0.05)
                .kd(0.0)
                .build();

        b.iter(|| {
            let process_var = black_box(85.0f64);
            black_box(pid.compute_correction(process_var))
        })
    });

    // Full PID controller (most complex)
    group.bench_function("pid_full", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.8)
                .ki(0.05)
                .kd(0.04)
                .build();

        b.iter(|| {
            let process_var = black_box(85.0f64);
            black_box(pid.compute_correction(process_var))
        })
    });

    // PID with all features (anti-windup, error bias, limits)
    group.bench_function("pid_full_featured", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.8)
                .ki(0.05)
                .kd(0.04)
                .error_bias(0.1)
                .error_limit(50.0)
                .output_limit(20.0)
                .build();

        b.iter(|| {
            let process_var = black_box(85.0f64);
            black_box(pid.compute_correction(process_var))
        })
    });

    group.finish();
}

/// Benchmark: PID behavior with different error magnitudes
///
/// Tests if PID computation time varies with error size.
/// Should be O(1) regardless of error magnitude.
fn pid_error_magnitude_impact(c: &mut Criterion) {
    let mut group = c.benchmark_group("error_magnitude");

    for error_magnitude in [1.0, 10.0, 100.0, 1000.0] {
        group.bench_with_input(
            BenchmarkId::new("error", error_magnitude),
            &error_magnitude,
            |b, &error| {
                let mut pid: nenya::pid_controller::PIDController<f64> =
                    PIDControllerBuilder::new(100.0f64)
                        .kp(0.8)
                        .ki(0.05)
                        .kd(0.04)
                        .build();

                // Create process variable that produces desired error
                let process_var = 100.0 - error;

                b.iter(|| black_box(pid.compute_correction(black_box(process_var))))
            },
        );
    }

    group.finish();
}

/// Benchmark: Anti-windup mechanism overhead
///
/// Measures the cost of anti-windup feedback when output is saturated.
fn anti_windup_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("anti_windup");

    // Without saturation (no anti-windup active)
    group.bench_function("no_saturation", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.8)
                .ki(0.05)
                .kd(0.04)
                .output_limit(50.0)
                .build();

        b.iter(|| black_box(pid.compute_correction(black_box(95.0f64)))) // Small error
    });

    // With saturation (anti-windup active)
    group.bench_function("with_saturation", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.8)
                .ki(0.05)
                .kd(0.04)
                .output_limit(5.0) // Low limit to force saturation
                .build();

        b.iter(|| black_box(pid.compute_correction(black_box(50.0f64)))) // Large error
    });

    group.finish();
}

/// Benchmark: Error bias computation overhead
///
/// Measures the cost of asymmetric error handling via error bias.
fn error_bias_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("error_bias");

    // Without error bias (baseline)
    group.bench_function("no_bias", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.8)
                .ki(0.05)
                .kd(0.04)
                .build();

        b.iter(|| black_box(pid.compute_correction(black_box(85.0f64))))
    });

    // With positive error bias
    group.bench_function("positive_bias", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.8)
                .ki(0.05)
                .kd(0.04)
                .error_bias(0.2)
                .build();

        b.iter(|| black_box(pid.compute_correction(black_box(85.0f64))))
    });

    // With negative error bias
    group.bench_function("negative_bias", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.8)
                .ki(0.05)
                .kd(0.04)
                .error_bias(-0.2)
                .build();

        b.iter(|| black_box(pid.compute_correction(black_box(85.0f64))))
    });

    group.finish();
}

/// Benchmark: Sustained computation (integral windup test)
///
/// Runs many consecutive PID computations to ensure performance
/// doesn't degrade as integral term accumulates.
fn sustained_computation(c: &mut Criterion) {
    let mut group = c.benchmark_group("sustained");

    // With error limit (bounded integral)
    group.bench_function("with_error_limit", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.8)
                .ki(0.05)
                .kd(0.04)
                .error_limit(50.0)
                .build();

        b.iter(|| {
            for _ in 0..1000 {
                black_box(pid.compute_correction(black_box(50.0f64))); // Sustained error
            }
        })
    });

    // Without error limit (unbounded integral)
    group.bench_function("without_error_limit", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.8)
                .ki(0.05)
                .kd(0.04)
                .build();

        b.iter(|| {
            for _ in 0..1000 {
                black_box(pid.compute_correction(black_box(50.0f64))); // Sustained error
            }
        })
    });

    group.finish();
}

/// Benchmark: Different numeric types (f32 vs f64)
///
/// Compares performance between f32 and f64.
/// Library is generic over Float types, so this helps choose defaults.
fn numeric_type_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("numeric_types");

    // f64 (default, higher precision)
    group.bench_function("f64", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.8)
                .ki(0.05)
                .kd(0.04)
                .build();

        b.iter(|| black_box(pid.compute_correction(black_box(85.0f64))))
    });

    // f32 (lower precision, potentially faster)
    group.bench_function("f32", |b| {
        let mut pid: nenya::pid_controller::PIDController<f32> =
            PIDControllerBuilder::new(100.0f32)
                .kp(0.8)
                .ki(0.05)
                .kd(0.04)
                .build();

        b.iter(|| black_box(pid.compute_correction(black_box(85.0f32))))
    });

    group.finish();
}

/// Benchmark: Varying setpoints
///
/// Tests if PID computation is affected by the magnitude of the setpoint.
/// Should be O(1) regardless of setpoint value.
fn varying_setpoints(c: &mut Criterion) {
    let mut group = c.benchmark_group("setpoints");

    for setpoint in [10.0, 100.0, 1000.0, 10_000.0, 100_000.0] {
        group.bench_with_input(
            BenchmarkId::new("setpoint", setpoint),
            &setpoint,
            |b, &sp| {
                let mut pid: nenya::pid_controller::PIDController<f64> =
                    PIDControllerBuilder::new(sp)
                        .kp(0.8)
                        .ki(0.05)
                        .kd(0.04)
                        .build();

                let process_var = sp * 0.85; // 15% below setpoint

                b.iter(|| black_box(pid.compute_correction(black_box(process_var))))
            },
        );
    }

    group.finish();
}

/// Benchmark: Extreme parameter values
///
/// Tests PID computation with very large or very small gain values
/// to ensure numerical stability and consistent performance.
fn extreme_parameters(c: &mut Criterion) {
    let mut group = c.benchmark_group("extreme_params");

    // Very small gains
    group.bench_function("small_gains", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(0.001)
                .ki(0.0001)
                .kd(0.0001)
                .build();

        b.iter(|| black_box(pid.compute_correction(black_box(85.0f64))))
    });

    // Very large gains
    group.bench_function("large_gains", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(10.0)
                .ki(1.0)
                .kd(1.0)
                .output_limit(1000.0)
                .build();

        b.iter(|| black_box(pid.compute_correction(black_box(85.0f64))))
    });

    // Mixed extreme values
    group.bench_function("mixed_extreme", |b| {
        let mut pid: nenya::pid_controller::PIDController<f64> =
            PIDControllerBuilder::new(100.0f64)
                .kp(10.0)
                .ki(0.0001)
                .kd(1.0)
                .output_limit(1000.0)
                .build();

        b.iter(|| black_box(pid.compute_correction(black_box(85.0f64))))
    });

    group.finish();
}

criterion_group!(
    benches,
    single_pid_computation,
    pid_error_magnitude_impact,
    anti_windup_overhead,
    error_bias_overhead,
    sustained_computation,
    numeric_type_comparison,
    varying_setpoints,
    extreme_parameters,
);

criterion_main!(benches);
