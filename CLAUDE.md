# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Nenya is a distributed adaptive rate limiter using PID (Proportional-Integral-Derivative) control and gossip-based coordination.

**Components**:
- **nenya**: Core Rust library providing adaptive rate limiting with PID control (no distributed features)
- **nenya-sentinel**: Standalone binary/sidecar that adds distributed coordination via gossip protocol

**Vision**: A "one-click" sidecar for microservices that provides distributed rate limiting with minimal configuration. Applications simply call a local HTTP endpoint to make throttling decisions.

## Architecture & Roadmap

**Detailed documentation**:
- [`docs/architecture.md`](docs/architecture.md) - Complete architectural design, component details, configuration
- [`docs/roadmap.md`](docs/roadmap.md) - Phased implementation plan with concrete tasks and milestones

**Key architectural decisions**:
- HTTP/JSON API (not gRPC) for simplicity and cross-language support
- Chitchat library for gossip protocol (Scuttlebutt algorithm, better reliability than SWIM)
- Pattern-based scope configuration with auto-creation
- Cluster secret authentication for security
- Pluggable discovery (Docker Swarm, Kubernetes, static seeds)

## Development Commands

### Building and Testing

```bash
# Build the entire workspace
cargo build

# Build a specific crate
cargo build -p nenya
cargo build -p nenya-sentinel

# Run all tests
cargo test --verbose

# Run tests for a specific crate
cargo test -p nenya

# Run a specific test
cargo test -p nenya test_name

# Run integration tests only
cargo test --test '*'
```

### Code Quality

```bash
# Format code
cargo fmt

# Check formatting without making changes
cargo fmt -- --check

# Run clippy linter
cargo clippy --all-targets --all-features -- -D warnings

# Security audit
cargo audit
```

### Examples

```bash
# Run the request simulator with plotting
cargo run --example request_simulator_plot -- \
    --target_tps 80.0 \
    --min_tps 75.0 \
    --max_tps 100.0 \
    --duration 120 \
    --kp 0.8 \
    --ki 0.05 \
    --kd 0.04

# See all available options
cargo run --example request_simulator_plot -- --help
```

### Documentation

```bash
# Generate and view documentation locally
cargo doc --no-deps --open
```

## Codebase Structure

### nenya/ (Core Library)

**Current implementation** (no changes needed for distributed features):

- `src/lib.rs` - RateLimiter with sliding window + PID integration
  - Generic over `T: Float + Signed + FromPrimitive`
  - Builder pattern: `RateLimiterBuilder`
  - External rate injection: `set_external_request_rate()`, `set_external_accepted_request_rate()`

- `src/pid_controller.rs` - PID control algorithm
  - Error bias for asymmetric response
  - Integral windup prevention
  - Anti-windup feedback
  - Builder pattern: `PIDControllerBuilder`

- `examples/` - Request simulator for testing and tuning

**Key patterns**:
- Sliding window: `VecDeque<Instant>` for request timestamps, trimmed by `update_interval`
- PID loop: Error → P/I/D terms → Clamping → Anti-windup → Correction
- External rate injection enables distributed coordination (used by nenya-sentinel)

### nenya-sentinel/ (Distributed Binary)

**Current state**: Basic gRPC skeleton (to be replaced with HTTP)

**Planned structure** (see roadmap Phase 0):
```
nenya-sentinel/
├── src/
│   ├── main.rs              # Entry point, setup, graceful shutdown
│   ├── api/                 # HTTP API handlers
│   │   ├── mod.rs
│   │   ├── throttle.rs      # POST /should_throttle
│   │   ├── health.rs        # GET /health
│   │   └── metrics.rs       # GET /metrics
│   ├── manager/             # Rate limit manager
│   │   ├── mod.rs
│   │   └── pattern.rs       # Scope pattern matching
│   ├── gossip/              # Gossip protocol integration
│   │   ├── mod.rs
│   │   └── state.rs         # Cluster state aggregation
│   ├── discovery/           # Peer discovery
│   │   ├── mod.rs
│   │   ├── static.rs
│   │   ├── docker_swarm.rs
│   │   └── kubernetes.rs
│   ├── config/              # Configuration loading
│   │   └── mod.rs
│   └── observability/       # Metrics & tracing
│       └── mod.rs
├── tests/
│   └── integration/
│       ├── cluster.rs       # Multi-node tests
│       └── helpers.rs       # Test harness
└── Cargo.toml
```

### Workspace

- `Cargo.toml` (root) - Workspace definition with shared metadata
- `.github/workflows/rust.yml` - CI/CD pipeline (test, fmt, clippy, audit)

## Key Implementation Details

### nenya Library

**Generic Numeric Types**:
- Generic over `T: Float + Signed + FromPrimitive + Copy`
- Enables f32/f64 tradeoffs
- All conversions use `num_traits::from_*` for safety

**Time Handling**:
- `std::time::Instant` for monotonic timestamps
- Minimum duration threshold (0.1s) prevents division by tiny numbers

**External Rate Injection** (critical for distribution):
- `set_external_request_rate(rate)` - Add remote request rates
- `set_external_accepted_request_rate(rate)` - Add remote accepted rates
- RateLimiter sums local + external rates for PID control
- This is how nenya-sentinel coordinates across nodes

**PID Controller**:
- Setpoint = target request rate (e.g., 100 RPS)
- Process variable = actual request rate (measured via sliding window)
- Error = setpoint - actual
- Output = correction to apply to target rate
- Clamped to min_rate/max_rate bounds

### nenya-sentinel (Future)

**Multi-Tenancy**:
- `HashMap<String, RateLimiter<f64>>` - One limiter per scope
- Scopes auto-created on first use
- Pattern matching: `api#*` matches `api#key123`
- Each scope has independent PID controller

**Distributed Coordination**:
1. Local RateLimiter tracks local request rate
2. Gossip protocol shares rates with peers
3. Manager aggregates peer rates: `sum(peer.scope.accepted_rate)`
4. Sets external_rate on local limiter: `limiter.set_external_request_rate(sum)`
5. PID controller adjusts based on total (local + remote) rate

**Configuration Hierarchy**:
1. Hardcoded defaults
2. TOML file (`./nenya.toml` or `/etc/nenya/nenya.toml`)
3. Environment variable overrides
4. Command-line flags (optional)

## Testing Philosophy

**Current tests** (nenya library):
- Unit tests in `src/lib.rs::tests` and `src/pid_controller.rs::tests`
- Focus on correctness of rate limiting and PID algorithms
- Time-dependent tests use `sleep()` for real behavior

**Future tests** (nenya-sentinel):
- Integration tests spawn real processes
- Must be deterministic and non-interactive
- Runnable in CI/CD via `cargo test`
- Network partition simulation
- Multi-node coordination verification

**CI/CD Requirements**:
- All tests must pass before merge
- No test flakiness tolerated
- Fast execution (<30s for full suite preferred)

## Development Workflow

**Current Status**: Architecture and planning complete

**Roadmap**: See [`docs/roadmap.md`](docs/roadmap.md) for the complete implementation plan

### Iterative Development Process

1. **Check current milestone**: Open `docs/roadmap.md` and find the "Current Milestone" section
2. **Review architecture**: Read the relevant section in `docs/architecture.md` (referenced in milestone)
3. **Develop plan**: Use plan mode if needed to break down tasks
4. **Implement with tests**: Write tests alongside code
5. **Verify milestone complete**: Run verification commands from roadmap
   ```bash
   cargo test          # All tests pass
   cargo fmt --check   # Code formatted
   cargo clippy        # No warnings
   ```
6. **Commit and push**: Use the suggested commit message from roadmap
7. **Check off milestone**: Mark `[x] MILESTONE COMPLETE` in `docs/roadmap.md`
8. **Update status**: Change "Current Milestone" to next milestone
9. **Move to next milestone**: Repeat process

### Milestone Overview

| Milestone | Status | Deliverable |
|-----------|--------|-------------|
| 0 | ⏳ Current | Clean HTTP stack |
| 1 | 🔜 Next | Working HTTP rate limiter |
| 2 | 🔜 Future | Distributed coordination |
| 3 | 🔜 Future | Platform integrations |
| 4 | 🔜 Future | Cluster authentication |
| 5 | 🔜 Future | Production-ready v1.0.0 |

See `docs/roadmap.md` for complete details on each milestone.

### Quick Reference

**Before starting work**:
- Check `docs/roadmap.md` for current milestone and tasks
- Review `docs/architecture.md` for design context

**While working**:
- Follow the task checklist in roadmap
- Write tests alongside implementation
- Reference architecture doc for design decisions

**After completing milestone**:
- Run verification commands from roadmap
- Commit with suggested message
- Update roadmap status

## Dependencies

### nenya
- `num-traits` - Generic numeric operations
- `log` - Logging (simple, library-friendly)

### nenya-sentinel (planned)
- `axum` - HTTP framework
- `tokio` - Async runtime
- `serde`, `serde_json` - JSON serialization
- `chitchat` - Gossip protocol (Scuttlebutt)
- `tracing`, `tracing-subscriber` - Observability
- `metrics`, `metrics-exporter-prometheus` - Metrics
- `toml` - Configuration parsing
- Platform-specific:
  - `bollard` - Docker API client (for Swarm discovery)
  - `kube` - Kubernetes API client (for K8s discovery)

## Important Constraints

**No backwards compatibility required**: Project is 0.x, no users yet, breaking changes are fine

**No gRPC**: HTTP/JSON for simplicity and universal language support

**Security model**: Cluster secret required, gossip authenticated, discovery is unauthenticated (just finds candidates)

**Failure modes**: Fail gracefully with stale data during partitions, recover automatically when healed

**Platform support**: Docker Swarm, Kubernetes, bare metal/VMs (no AWS Lambda for now)

## Common Patterns

**Builder pattern**:
```rust
let limiter = RateLimiterBuilder::new(100.0)
    .min_rate(50.0)
    .max_rate(200.0)
    .pid_controller(pid)
    .build();
```

**Pattern matching for scopes**:
```toml
[[rate_limits]]
pattern = "api#premium_*"
target_rate = 1000.0

[[rate_limits]]
pattern = "api#*"
target_rate = 100.0

[[rate_limits]]
pattern = "*"
target_rate = 10.0
```
Priority: exact match > most specific pattern > default

**Error handling**: Use `Result<T, Error>` with `anyhow` or custom error types, never panic in library code

**Logging**: Use `tracing` macros (`tracing::info!`, `tracing::error!`) with `#[instrument]` for spans

## References

- **SWIM Protocol**: "SWIM: Scalable Weakly-consistent Infection-style Process Group Membership Protocol"
- **Scuttlebutt**: "Efficient Reconciliation and Flow Control for Anti-Entropy Protocols"
- **Chitchat**: https://quickwit.io/blog/chitchat
- **PID Control**: Standard control theory, see README.md for algorithm details
