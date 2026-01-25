[![Rust Build](https://github.com/emersonmde/nenya/actions/workflows/rust.yml/badge.svg)](https://github.com/emersonmde/nenya/actions/workflows/rust.yml)
[![Docs](https://img.shields.io/docsrs/nenya/latest)](https://docs.rs/nenya)
[![crates](https://img.shields.io/crates/v/nenya.svg)](https://crates.io/crates/nenya)
[![License](https://img.shields.io/crates/l/nenya.svg)](LICENSE)

# Nenya

**Nenya** is an adaptive rate limiter using a Proportional-Integral-Derivative (PID) controller.

**Two ways to use it:**
- **As a library**: Embedded rate limiting in your Rust application
- **As a binary**: Distributed rate limiting sidecar for microservices (work in progress)

## Features

- **Adaptive PID Control**: Dynamically adjusts rate limits based on traffic patterns
- **Token Bucket + Sliding Window Hybrid**: Precise throttling with accurate rate measurement
- **Timestamp Collision Immunity**: Handles tight-loop scenarios without rate calculation artifacts
- **Generic Implementation**: Works with any numeric type (f32, f64, etc.)
- **Distributed Coordination**: Share rate limits across multiple instances (coming soon)

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

Then run:
```bash
nenya
```

**Note:** The distributed sidecar is under active development. See [docs/roadmap.md](docs/roadmap.md) for progress.

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

### Request Simulator

Nenya includes a request simulation example for testing and tuning. You can
run the simulation with:

```sh
cargo run --example request_simulator_plot -- \
    --target_tps 80.0 \
    --min_tps 75.0 \
    --max_tps 100.0 \
    --trailing_window 1 \
    --duration 120 \
    --base_tps 80.0 \
    --amplitudes 20.0,7.0,10.0 \
    --frequencies 0.05,2.8,4.0 \
    --kp 0.8 \
    --ki 0.05 \
    --kd 0.04 \
    --error_limit 10.0 \
    --output_limit 3.0 \
    --update_interval 500 \
    --error_bias 0.0

```

Most of these arguments have sane defaults and can be omitted. For more details
see:

```sh
cargo run --example request_simulator_plot -- --help
```

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

## Adaptive Rate Limiting

The rate limiter achieves an adaptive rate limit using a
Proportional–Integral–Derivative (PID) controller which determines the target
rate limit based on the request rate. This implementation includes error
bias, accumulated error clamping, anti-windup feedback, and output clamping.

### Overview

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
