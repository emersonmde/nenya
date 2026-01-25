# Performance Benchmarks

Criterion-based benchmarks for measuring the performance of the nenya rate limiter library.

> **Note**: These benchmarks establish performance baselines and catch regressions. For comprehensive testing including load tests and distributed scenarios, see the **Testing Requirements** sections in [`docs/roadmap.md`](../docs/roadmap.md) under each milestone.

## Quick Start

### Running Benchmarks

```bash
# Run ALL benchmarks (both suites, all tests)
cargo bench

# Run specific benchmark suite
cargo bench --bench rate_limiter_bench
cargo bench --bench pid_controller_bench

# Run specific test group within a suite
cargo bench --bench rate_limiter_bench -- hot_path
cargo bench --bench pid_controller_bench -- pid_computation

# Run a single specific test
cargo bench --bench rate_limiter_bench -- hot_path/warm_steady_state
cargo bench --bench pid_controller_bench -- pid_computation/pid_full
```

### Saving Baselines and Comparing Performance

**Establishing a baseline** (do this before making changes):

```bash
# Save current performance as baseline named "main"
cargo bench -- --save-baseline main

# Or save with a descriptive name
cargo bench -- --save-baseline before-optimization
```

**Comparing after changes:**

```bash
# Make your code changes...
# Then compare against the baseline
cargo bench -- --baseline main

# You'll see output like:
#   time:   [40.238 ns 40.381 ns 40.520 ns]
#   change: [-5.2341% -3.1234% -0.8821%] (improvement)
```

**Rerunning all benchmarks from scratch:**

```bash
# Delete previous results and run fresh
rm -rf target/criterion
cargo bench

# Or force re-run without cache
cargo clean
cargo bench
```

### Common Workflows

**Pre-commit performance check:**
```bash
# Before starting work
cargo bench -- --save-baseline before-changes

# After making changes
cargo bench -- --baseline before-changes

# Look for regressions (red "regressed" indicators)
```

**Tracking performance across branches:**
```bash
# On main branch
git checkout main
cargo bench -- --save-baseline main

# On feature branch
git checkout feature/optimization
cargo bench -- --baseline main

# Compare results
```

**Quick smoke test (faster, less accurate):**
```bash
# Run with fewer samples for quick feedback
cargo bench -- --quick

# Run just the hot path
cargo bench -- hot_path/warm_steady_state
```

## Benchmark Suites

### rate_limiter_bench.rs

**Critical hot path benchmarks** - These measure the per-request overhead:

- `hot_path` - Single throttling decision latency (**target: <1μs p99**)
  - `cold_start` - First decision ever
  - `warm_steady_state` - Typical steady-state operation
  - `burst_100_decisions` - Rapid burst handling

- `throughput` - Decisions per second at different scales (**target: >1M/sec**)
  - Tests at 100, 1K, 10K, 100K TPS

- `sliding_window` - Window maintenance overhead
  - Tests with 10 to 10,000 entries

- `time_control` - Overhead of time injection for tests

- `pid_update_frequency` - Impact of PID update interval

- `external_rates` - Distributed coordination overhead

- `memory` - Sustained load for allocation profiling

- `realistic` - Mixed workload simulation

### pid_controller_bench.rs

**PID computation overhead** - These measure the control algorithm cost:

- `pid_computation` - Single correction calculation (**target: <100ns**)
  - P-only, PI, PID, and full-featured variants

- `error_magnitude` - Verify O(1) complexity

- `anti_windup` - Saturation handling overhead

- `error_bias` - Asymmetric error handling cost

- `sustained` - Long-running stability check

- `numeric_types` - f32 vs f64 performance

- `setpoints` - Different scale handling

- `extreme_params` - Numerical stability

## Understanding Benchmark Output

### Reading the Results

When you run a benchmark, Criterion shows detailed statistics:

```
hot_path/warm_steady_state
                        time:   [40.238 ns 40.381 ns 40.520 ns]
                        change: [-2.1234% -0.8821% +0.5432%] (p = 0.23 > 0.05)
                        No change in performance detected.
Found 3 outliers among 100 measurements (3.00%)
  2 (2.00%) low mild
  1 (1.00%) high mild
```

**What each line means:**
- **time**: `[lower_bound estimate upper_bound]` - 95% confidence interval
  - `estimate` (middle value) is the best measurement
  - Bounds show measurement uncertainty

- **change**: Comparison to baseline (only shown with `--baseline`)
  - Negative % = improvement (faster)
  - Positive % = regression (slower)
  - `(p = 0.23 > 0.05)` = not statistically significant

- **Performance verdict**:
  - `"Performance has improved"` (green) = significantly faster
  - `"Performance has regressed"` (red) = significantly slower
  - `"No change detected"` = within noise/uncertainty

- **Outliers**: Measurements far from typical
  - A few outliers are normal (OS interrupts, cache misses)
  - Many outliers suggest unstable benchmark

### Example: Good Improvement

```
time:   [38.123 ns 38.456 ns 38.789 ns]
change: [-5.2341% -3.1234% -0.8821%] (p = 0.00 < 0.05)
Performance has improved.
```
✅ Definitely faster! The code is 3% faster on average.

### Example: Regression Detected

```
time:   [45.234 ns 46.123 ns 47.012 ns]
change: [+10.123% +12.456% +14.789%] (p = 0.00 < 0.05)
Performance has regressed.
```
❌ Slower! The code is ~12% slower - investigate what changed.

### Example: No Significant Change

```
time:   [40.100 ns 40.381 ns 40.662 ns]
change: [-1.234% -0.123% +0.987%] (p = 0.67 > 0.05)
No change in performance detected.
```
➖ Noise - the change is too small to be sure it's real.

### HTML Reports

Criterion generates detailed HTML reports in `target/criterion/`:

```bash
# Run benchmarks
cargo bench

# Open report in browser (macOS)
open target/criterion/report/index.html

# Or navigate to specific benchmark
open target/criterion/hot_path/warm_steady_state/report/index.html
```

Reports include:
- **Violin plots**: Distribution of measurements
- **Line charts**: Performance over time
- **PDF/CDF plots**: Statistical distribution
- **Comparison charts**: Before/after when using baselines

## Interpreting Results

### Key Metrics

**Decision Latency (hot_path/warm_steady_state)**:
- **Target**: <1μs p99
- **Acceptable**: <5μs p99
- **Action if exceeded**: Profile with `cargo flamegraph` or `perf`

**Throughput (throughput/decisions_per_sec)**:
- **Target**: >1M decisions/sec on single thread
- **Acceptable**: >500K decisions/sec
- **Action if low**: Check for allocations, expensive operations

**PID Computation (pid_computation/pid_full)**:
- **Target**: <100ns per computation
- **Acceptable**: <500ns
- **Action if slow**: Optimize math operations, check for divisions

### Performance Budget

For a 100ms request budget:
- Rate limiter decision: <1ms (1%)
- HTTP framework overhead: <5ms (5%)
- Gossip coordination: <5ms (5%)
- Application logic: ~89ms (89%)

### Red Flags

⚠️ **Memory allocations in hot path**: Run with `RUSTFLAGS="-Z print-type-sizes"` or valgrind
⚠️ **Non-linear scaling**: If 100K TPS is >10x slower than 10K TPS
⚠️ **High variance**: p99 > 10x mean suggests unpredictable behavior

## Advanced Profiling

### Flamegraphs (Linux/macOS)

```bash
cargo install flamegraph
cargo flamegraph --bench rate_limiter_bench -- --bench hot_path
```

### Memory Profiling

```bash
# Valgrind (Linux)
valgrind --tool=massif --massif-out-file=massif.out \
    target/release/deps/rate_limiter_bench-* --bench hot_path
ms_print massif.out

# Instruments (macOS)
cargo instruments --bench rate_limiter_bench --template Allocations
```

### CPU Profiling

```bash
# perf (Linux)
cargo build --release --bench rate_limiter_bench
perf record -g target/release/deps/rate_limiter_bench-* --bench hot_path
perf report

# Instruments (macOS)
cargo instruments --bench rate_limiter_bench --template Time
```

## Baseline Tracking

Track performance regressions across commits:

```bash
# Before optimization
git checkout main
cargo bench -- --save-baseline main

# After optimization
git checkout feature-branch
cargo bench -- --baseline main

# Look for:
# - "improved" in green (faster)
# - "regressed" in red (slower)
# - "no change" (within noise threshold)
```

## Continuous Integration

Add to CI pipeline:

```yaml
# .github/workflows/bench.yml
- name: Run benchmarks
  run: cargo bench --no-fail-fast

- name: Compare with main
  run: |
    git fetch origin main
    git checkout origin/main
    cargo bench -- --save-baseline main
    git checkout -
    cargo bench -- --baseline main
```

## Writing New Benchmarks

When adding features, add corresponding benchmarks:

1. **Hot path changes**: Add to `rate_limiter_bench.rs`
2. **PID algorithm changes**: Add to `pid_controller_bench.rs`
3. **New features**: Create new benchmark file and add to `Cargo.toml`

### Benchmark Template

```rust
fn my_new_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("my_feature");

    // Setup
    let mut limiter = /* ... */;

    // Benchmark
    group.bench_function("description", |b| {
        b.iter(|| {
            black_box(limiter.some_operation())
        })
    });

    group.finish();
}
```

## Performance Targets by Use Case

### High-Frequency Trading / Real-Time Systems
- Decision latency: <100ns p99
- Throughput: >10M/sec

### High-Traffic Web Services (target for nenya)
- Decision latency: <1μs p99
- Throughput: >1M/sec

### Standard Microservices
- Decision latency: <10μs p99
- Throughput: >100K/sec

### Low-Traffic Services
- Decision latency: <100μs p99
- Throughput: >10K/sec
