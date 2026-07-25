[![Rust Build](https://github.com/emersonmde/nenya/actions/workflows/rust.yml/badge.svg)](https://github.com/emersonmde/nenya/actions/workflows/rust.yml)
[![Docs](https://img.shields.io/docsrs/nenya/latest)](https://docs.rs/nenya)
[![crates](https://img.shields.io/crates/v/nenya.svg)](https://crates.io/crates/nenya)
[![License](https://img.shields.io/crates/l/nenya.svg)](LICENSE)

# Nenya

**Nenya** is an adaptive rate limiter using a Proportional-Integral-Derivative (PID) controller.

**Two ways to use it:**
- **As a library**: Embedded rate limiting in your Rust application
- **As a binary**: Distributed rate limiting sidecar for microservices

## Features

- **PID-based adaptive control**: Adjusts rate limits in real-time based on measured throughput
- **Distributed coordination**: Equal division PID algorithm for cluster-wide rate limiting
- **Pluggable control engines**: PID (default), Bayesian (per-peer Kalman
  estimation with uncertainty-aware admission), and a Kalman→PID hybrid —
  explicit config, benchmarked head-to-head in
  [docs/engine-comparison.md](docs/engine-comparison.md)
- **Token bucket + sliding window hybrid**: Fast per-request decisions with accurate rate measurement
- **AIMD-inspired tuning**: Conservative PID gains optimized for distributed systems (kp=0.5, ki=0.02, kd=0.08)
- **Generic over numeric types**: Works with f32, f64, or custom numeric types

## Installation

### As a Library

Add to your `Cargo.toml`:

```toml
[dependencies]
nenya = "0.1"
```

### As a Binary (Sidecar)

```bash
cargo install nenya
```

Run a single node:
```bash
NENYA_CLUSTER_SECRET=your-secret nenya
```

Run a 3-node cluster:
```bash
# Node 0 (seed)
NENYA_CLUSTER_SECRET=secret \
NENYA_LISTEN_ADDR=127.0.0.1:8080 \
NENYA_GOSSIP_ADDR=127.0.0.1:8081 \
NENYA_ENABLE_GOSSIP=1 \
nenya

# Node 1
NENYA_CLUSTER_SECRET=secret \
NENYA_LISTEN_ADDR=127.0.0.1:8090 \
NENYA_GOSSIP_ADDR=127.0.0.1:8091 \
NENYA_SEED_NODES=127.0.0.1:8081 \
nenya

# Node 2
NENYA_CLUSTER_SECRET=secret \
NENYA_LISTEN_ADDR=127.0.0.1:8100 \
NENYA_GOSSIP_ADDR=127.0.0.1:8101 \
NENYA_SEED_NODES=127.0.0.1:8081 \
nenya
```

**Status:** HTTP API and distributed gossip coordination complete. See [docs/roadmap.md](docs/roadmap.md) for upcoming features.

### Examples

A basic rate limiter with a static set point:

```rust
use nenya::RateLimiterBuilder;
use nenya::pid_controller::PIDControllerBuilder;
use std::time::Duration;

fn main() {
    // Create a rate limiter
    let mut rate_limiter = RateLimiterBuilder::new(10.0)
        .update_interval(Duration::from_secs(1))
        .build();

    // Simulate request processing and check if throttling is necessary
    for _ in 0..20 {
        if rate_limiter.should_throttle() {
            println!("Request throttled");
        } else {
            println!("Request accepted");
        }
    }
}
```

A dynamic rate limiter using a PID Controller:

```rust
use nenya::RateLimiterBuilder;
use nenya::pid_controller::PIDControllerBuilder;
use std::time::Duration;

fn main() {
    // Create a PID controller with specific parameters
    let pid_controller = PIDControllerBuilder::new(10.0)
        .kp(1.0)
        .ki(0.1)
        .kd(0.01)
        .build();

    // Create a rate limiter using the PID Controller
    let mut rate_limiter = RateLimiterBuilder::new(10.0)
        .min_rate(5.0)
        .max_rate(15.0)
        .pid_controller(pid_controller)
        .update_interval(Duration::from_secs(1))
        .build();

    // Simulate request processing and check if throttling is necessary
    for _ in 0..20 {
        if rate_limiter.should_throttle() {
            println!("Request throttled");
        } else {
            println!("Request accepted");
        }
    }
}
```

Distributed rate limiting across a cluster:

```rust
use nenya::RateLimiterBuilder;
use nenya::pid_controller::PIDControllerBuilder;

fn main() {
    let pid = PIDControllerBuilder::new(0.0)  // Setpoint adjusted automatically
        .kp(0.8)
        .ki(0.05)
        .kd(0.04)
        .build();

    let mut limiter = RateLimiterBuilder::new(100.0)
        .cluster_target(1000.0)  // 1000 RPS cluster-wide target
        .min_rate(50.0)
        .max_rate(200.0)
        .pid_controller(pid)
        .build();

    // Update peer count from gossip protocol
    limiter.set_num_peers(9);  // 10 nodes total → 100 RPS per node

    // Inject aggregated rates from other nodes
    limiter.set_external_accepted_request_rate(850.0);

    if limiter.should_throttle() {
        println!("Request throttled");
    } else {
        println!("Request accepted");
    }
}
```

### Cluster Simulator

A deterministic multi-node simulator (feature `sim`) is the primary tool for
testing cluster dynamics: N in-process nodes with real rate limiters, a
message-bus gossip model (propagation delay, jitter, loss, partitions),
seeded workloads, and a virtual clock. A 60-second scenario runs in
milliseconds, and the same seed always produces byte-identical artifacts.

```sh
# List scenarios (steady state, step, ramp, burst, join/leave,
# partition + heal, skewed load, scale sweep, sinusoidal)
cargo run --features sim --example cluster_sim -- --list

# Run one scenario; writes CSV + JSON time series and an SVG chart
cargo run --features sim --example cluster_sim -- \
    --scenario partition --seed 42 --plot

# Run the full scenario matrix and print comparison tables (markdown) —
# one table per control engine (pid, bayesian, hybrid), or a single
# engine with --engine. Gains, the anti-windup clamp, and estimator
# parameters can be overridden per run for A/B comparisons
# (--kp/--ki/--kd/--error-limit-frac/--process-noise/--measurement-noise).
cargo run --features sim --example cluster_sim -- --matrix --seed 42
```

Artifacts land in `target/sim/` by default (`--out` to change). The
simulation test suite (`tests/simulation.rs`) asserts the roadmap's
acceptance thresholds — convergence within the ±5% band, bounded partition
overshoot, post-heal recovery — against these same scenarios in CI. Scaling
laws and sizing coefficients (nodes, scopes, rates, memory) are documented
in [docs/capacity-model.md](docs/capacity-model.md), re-derivable via the
`tests/capacity.rs` suite.

## Development

### Setup

Enable pre-commit hooks (runs tests, clippy, fmt, and audit before each commit):
```bash
git config core.hooksPath .git-hooks
```

### Running Checks

```bash
# Run all pre-commit checks manually
./.git-hooks/pre-commit

# Or run individual checks
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt -- --check
cargo audit
```

## How It Works

### Hybrid Architecture

Nenya combines three techniques:

1. **Token Bucket**: Fast per-request decisions, immune to timestamp collisions
2. **Sliding Window**: Accurate rate measurement for PID feedback
3. **PID Controller**: Adaptive adjustment based on actual vs target rate

### Single-Node Mode

Standard PID control loop:
- **Setpoint**: Target rate (e.g., 100 RPS)
- **Signal**: Measured accepted rate
- **Output**: Adjustment to token refill rate

### Distributed Mode (Equal Division PID)

For cluster-wide rate limiting:
- Each node gets an equal share: `cluster_target / num_nodes`
- Nodes exchange their accepted rates via gossip
- PID uses total cluster rate (local + remote) as feedback signal
- Automatically rebalances when nodes join/leave

Example: 1000 RPS cluster target with 10 nodes → each node targets 100 RPS. If a node sees the cluster is accepting 1100 RPS total, it reduces its local target proportionally.

### PID Algorithm

1. **Error Calculation**: The error is calculated by subtracting the request
   rate from the setpoint.
2. **Proportional Term**: The proportional term is the product of the
   proportional gain and the error.
3. **Error Bias**:  The error is adjusted by a bias factor, reacting more to
   positive errors if $B > 0$ and more to negative errors if $B < 0$.
4. **Integral Term**: The integral term is the accumulated error over time,
   clamped to prevent windup.
5. **Derivative Term**: The derivative term is the rate of change of the error.
6. **Raw Correction**: The raw correction is the sum of the P, I, and D terms.
7. **Output Clamping**: The output is clamped to a specified limit to prevent
   excessive corrections.
8. **Anti-Windup Feedback**: If clamping occurs, the accumulated error is
   adjusted to prevent windup.
9. **Final Output**: The clamped correction is the final output of the PID
   controller.
10. **Request Limit Adjustment**: The clamped correction is added to the
    current request limit to derive the new request limit.

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for more details.
