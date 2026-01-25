//! Performance benchmarks for RateLimiter
//!
//! These benchmarks measure the critical hot path: the time it takes to make
//! a throttling decision. This is the most performance-sensitive operation since
//! it happens on every request.
//!
//! ## Key Metrics
//!
//! - **Decision latency**: Time from `should_throttle()` to return (target: <1μs p99)
//! - **Throughput**: Decisions per second on single thread (target: >1M/sec)
//! - **Memory**: Allocations per decision (target: zero in steady state)
//!
//! ## Running Benchmarks
//!
//! ```bash
//! # Run all rate limiter benchmarks
//! cargo bench --bench rate_limiter_bench
//!
//! # Run specific benchmark
//! cargo bench --bench rate_limiter_bench -- hot_path
//!
//! # Compare with baseline
//! cargo bench --bench rate_limiter_bench -- --save-baseline main
//! # ... make changes ...
//! cargo bench --bench rate_limiter_bench -- --baseline main
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nenya::pid_controller::PIDControllerBuilder;
use nenya::RateLimiterBuilder;
use std::time::{Duration, Instant};

/// Benchmark: Hot path - single throttling decision
///
/// This is THE critical benchmark. Every request in production goes through this path.
/// Measures the time to make a single accept/reject decision.
fn hot_path_single_decision(c: &mut Criterion) {
    let mut group = c.benchmark_group("hot_path");

    // Cold start: Very first decision
    group.bench_function("cold_start", |b| {
        b.iter_batched(
            || {
                let pid = PIDControllerBuilder::new(100.0)
                    .kp(0.8)
                    .ki(0.05)
                    .kd(0.04)
                    .build();
                RateLimiterBuilder::new(100.0)
                    .min_rate(50.0)
                    .max_rate(150.0)
                    .pid_controller(pid)
                    .build()
            },
            |mut limiter| black_box(limiter.should_throttle()),
            criterion::BatchSize::SmallInput,
        )
    });

    // Warm cache: Steady-state operation (most realistic)
    group.bench_function("warm_steady_state", |b| {
        let pid = PIDControllerBuilder::new(100.0)
            .kp(0.8)
            .ki(0.05)
            .kd(0.04)
            .build();
        let mut limiter = RateLimiterBuilder::new(100.0)
            .min_rate(50.0)
            .max_rate(150.0)
            .pid_controller(pid)
            .build();

        // Warm up: Fill sliding window with some history
        for _ in 0..100 {
            limiter.should_throttle();
            std::thread::sleep(Duration::from_micros(100));
        }

        b.iter(|| black_box(limiter.should_throttle()))
    });

    // Under load: Many rapid decisions
    group.bench_function("burst_100_decisions", |b| {
        let pid = PIDControllerBuilder::new(100.0)
            .kp(0.8)
            .ki(0.05)
            .kd(0.04)
            .build();
        let mut limiter = RateLimiterBuilder::new(100.0)
            .min_rate(50.0)
            .max_rate(150.0)
            .pid_controller(pid)
            .build();

        b.iter(|| {
            for _ in 0..100 {
                black_box(limiter.should_throttle());
            }
        })
    });

    group.finish();
}

/// Benchmark: Throughput at different request rates
///
/// Measures how many decisions/sec the limiter can handle when simulating
/// different load levels (underload, at capacity, overload).
fn throughput_at_different_loads(c: &mut Criterion) {
    let mut group = c.benchmark_group("throughput");

    // Test at different TPS targets
    for target_tps in [100, 1000, 10_000, 100_000] {
        group.throughput(Throughput::Elements(target_tps as u64));

        group.bench_with_input(
            BenchmarkId::new("decisions_per_sec", target_tps),
            &target_tps,
            |b, &tps| {
                let pid = PIDControllerBuilder::new(tps as f64)
                    .kp(0.8)
                    .ki(0.05)
                    .kd(0.04)
                    .build();
                let mut limiter = RateLimiterBuilder::new(tps as f64)
                    .min_rate(tps as f64 * 0.5)
                    .max_rate(tps as f64 * 1.5)
                    .pid_controller(pid)
                    .build();

                b.iter(|| {
                    for _ in 0..1000 {
                        black_box(limiter.should_throttle());
                    }
                })
            },
        );
    }

    group.finish();
}

/// Benchmark: Sliding window operations
///
/// Measures the cost of window trimming and rate calculation as the
/// window grows. These operations happen periodically.
fn sliding_window_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("sliding_window");

    // Test with different window sizes
    for window_size in [10, 100, 1000, 10_000] {
        group.bench_with_input(
            BenchmarkId::new("trim_and_calculate", window_size),
            &window_size,
            |b, &size| {
                let pid = PIDControllerBuilder::new(100.0)
                    .kp(0.8)
                    .ki(0.05)
                    .kd(0.04)
                    .build();
                let base_time = Instant::now();
                let mut limiter = RateLimiterBuilder::new(100.0)
                    .min_rate(50.0)
                    .max_rate(150.0)
                    .pid_controller(pid)
                    .initial_timestamp(base_time - Duration::from_secs(10))
                    .build();

                // Fill window with history
                for i in 0..size {
                    let timestamp = base_time + Duration::from_micros(i * 100);
                    limiter.should_throttle_at(timestamp);
                }

                // Now measure the cost of decisions with a full window
                let current_time = base_time + Duration::from_secs(1);
                b.iter(|| black_box(limiter.should_throttle_at(current_time)))
            },
        );
    }

    group.finish();
}

/// Benchmark: Time-controlled decisions (testing path)
///
/// Measures overhead of `should_throttle_at()` vs `should_throttle()`.
/// This helps us understand the cost of time injection for tests.
fn time_controlled_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("time_control");

    let pid = PIDControllerBuilder::new(100.0)
        .kp(0.8)
        .ki(0.05)
        .kd(0.04)
        .build();
    let base_time = Instant::now();
    let mut limiter = RateLimiterBuilder::new(100.0)
        .min_rate(50.0)
        .max_rate(150.0)
        .pid_controller(pid)
        .initial_timestamp(base_time - Duration::from_secs(1))
        .build();

    group.bench_function("should_throttle_at", |b| {
        b.iter(|| {
            let now = Instant::now();
            black_box(limiter.should_throttle_at(now))
        })
    });

    group.bench_function("should_throttle", |b| {
        b.iter(|| black_box(limiter.should_throttle()))
    });

    group.finish();
}

/// Benchmark: PID update frequency impact
///
/// Measures how the PID update interval affects decision latency.
/// Shorter intervals mean more frequent PID computations during decisions.
fn pid_update_frequency_impact(c: &mut Criterion) {
    let mut group = c.benchmark_group("pid_update_frequency");

    for update_interval_ms in [10, 50, 100, 500, 1000] {
        group.bench_with_input(
            BenchmarkId::new("update_interval_ms", update_interval_ms),
            &update_interval_ms,
            |b, &interval_ms| {
                let pid = PIDControllerBuilder::new(100.0)
                    .kp(0.8)
                    .ki(0.05)
                    .kd(0.04)
                    .build();
                let base_time = Instant::now();
                let mut limiter = RateLimiterBuilder::new(100.0)
                    .min_rate(50.0)
                    .max_rate(150.0)
                    .pid_controller(pid)
                    .update_interval(Duration::from_millis(interval_ms))
                    .initial_timestamp(base_time - Duration::from_secs(1))
                    .build();

                // Simulate time progression to trigger PID updates
                let mut current_offset = 0u64;
                b.iter(|| {
                    current_offset += interval_ms;
                    let now = base_time + Duration::from_millis(current_offset);
                    black_box(limiter.should_throttle_at(now))
                })
            },
        );
    }

    group.finish();
}

/// Benchmark: External rate injection overhead
///
/// Measures the cost of external rate coordination (for distributed scenarios).
/// This simulates the overhead when using gossip to share rates.
fn external_rate_coordination_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("external_rates");

    // Without external rates (baseline)
    group.bench_function("no_external_rates", |b| {
        let pid = PIDControllerBuilder::new(100.0)
            .kp(0.8)
            .ki(0.05)
            .kd(0.04)
            .build();
        let mut limiter = RateLimiterBuilder::new(100.0)
            .min_rate(50.0)
            .max_rate(150.0)
            .pid_controller(pid)
            .build();

        b.iter(|| black_box(limiter.should_throttle()))
    });

    // With external rates (distributed coordination)
    group.bench_function("with_external_rates", |b| {
        let pid = PIDControllerBuilder::new(100.0)
            .kp(0.8)
            .ki(0.05)
            .kd(0.04)
            .build();
        let mut limiter = RateLimiterBuilder::new(100.0)
            .min_rate(50.0)
            .max_rate(150.0)
            .pid_controller(pid)
            .build();

        // Simulate external rates from 3 other nodes
        limiter.set_external_request_rate(30.0);
        limiter.set_external_accepted_request_rate(28.0);

        b.iter(|| black_box(limiter.should_throttle()))
    });

    group.finish();
}

/// Benchmark: Memory allocation patterns
///
/// While Criterion doesn't directly measure allocations, this benchmark
/// helps identify allocation-heavy code paths by running many iterations.
/// Use with valgrind or other profiling tools for detailed allocation analysis.
fn memory_allocation_stress(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory");
    group.measurement_time(Duration::from_secs(10)); // Longer run for profiling

    group.bench_function("sustained_load_10k_decisions", |b| {
        let pid = PIDControllerBuilder::new(100.0)
            .kp(0.8)
            .ki(0.05)
            .kd(0.04)
            .build();
        let mut limiter = RateLimiterBuilder::new(100.0)
            .min_rate(50.0)
            .max_rate(150.0)
            .pid_controller(pid)
            .build();

        b.iter(|| {
            for _ in 0..10_000 {
                black_box(limiter.should_throttle());
            }
        })
    });

    group.finish();
}

/// Benchmark: Realistic mixed workload
///
/// Simulates a realistic scenario with varying request patterns:
/// - Underload periods (accept most)
/// - At-capacity periods (careful throttling)
/// - Overload periods (reject most)
fn realistic_mixed_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("realistic");

    group.bench_function("mixed_workload", |b| {
        let pid = PIDControllerBuilder::new(100.0)
            .kp(0.8)
            .ki(0.05)
            .kd(0.04)
            .build();
        let base_time = Instant::now();
        let mut limiter = RateLimiterBuilder::new(100.0)
            .min_rate(50.0)
            .max_rate(150.0)
            .pid_controller(pid)
            .initial_timestamp(base_time - Duration::from_secs(1))
            .build();

        let mut tick = 0u64;

        b.iter(|| {
            // Varying decisions per tick to simulate load changes:
            // - 50% of ticks: 3 decisions (underload)
            // - 30% of ticks: 10 decisions (at capacity)
            // - 20% of ticks: 20 decisions (overload)
            let decisions_this_tick = match tick % 10 {
                0..=4 => 3,  // Underload
                5..=7 => 10, // At capacity
                _ => 20,     // Overload
            };

            for _ in 0..decisions_this_tick {
                black_box(limiter.should_throttle_at(base_time + Duration::from_millis(tick * 10)));
            }

            tick += 1;
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    hot_path_single_decision,
    throughput_at_different_loads,
    sliding_window_overhead,
    time_controlled_overhead,
    pid_update_frequency_impact,
    external_rate_coordination_overhead,
    memory_allocation_stress,
    realistic_mixed_workload,
);

criterion_main!(benches);
