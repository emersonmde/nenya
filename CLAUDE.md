# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Nenya is a distributed adaptive rate limiter. Nodes coordinate through a gossip
protocol (Chitchat/Scuttlebutt) by sharing only per-scope accepted rates; each
node runs a local control loop (currently PID) that converges the cluster toward
a global target. There is no central coordination service.

**Single crate, dual purpose**:
- **Library**: embedded rate limiting with PID control (`cargo add nenya`) — no server deps
- **Binary**: distributed rate limiting sidecar (`cargo install nenya`) — behind the `server` feature

**Vision**: a one-click sidecar for microservices. One pasted config block per
platform (Compose/Swarm/K8s/ECS), one guard clause (or thin SDK call) in the app.

**Positioning** (keep docs honest about this): gossip-based limits are *soft*.
Worst-case overshoot ≈ propagation_delay × excess demand. Nenya targets fairness
and overload protection, not billing-grade quota enforcement. Its differentiator
vs. Gubernator/Kong/Redis-based limiters is the control-theoretic approach with
no coordinator.

## Where to Start

1. **[`docs/roadmap.md`](docs/roadmap.md)** — check the "Current Milestone" section; this drives all work
2. **[`docs/architecture.md`](docs/architecture.md)** — design details; each section is marked Implemented or Planned
3. This file — commands, structure, conventions

**Current milestone: 7 — Client SDKs & API Stabilization.**
Stabilize the HTTP API (OpenAPI spec, versioning), then thin fail-open SDKs
for Rust/Python/Node/Go and cost-weighted rates (7.3). See the roadmap's
Milestone 7 section for the task list.

## Milestone Overview

| Milestone | Status | Deliverable |
|-----------|--------|-------------|
| 0-2 | ✅ Complete | Single-crate structure, HTTP rate limiter, gossip coordination |
| 3 | ✅ Complete | Gossip correctness fixes (stale decay, locking) |
| 4 | ✅ Complete | Deterministic multi-node simulator + scenario/benchmark suite |
| 5 | ✅ Complete | Pluggable engines (PID/Bayesian/hybrid) benchmarked; estimator-floor + cold-start fixes |
| 6 | ✅ Complete | Two-tier coordination for per-user scale (1M scopes at ~360 B each; sweep-derived promotion policy) |
| 7 | ⏳ Current | Client SDKs (Rust, Python, Node, Go) |
| 8 | 🔜 Future | Platform deployment + discovery + AgentCore quota arbitration |
| 9 | 🔜 Future | Cluster authentication |
| 10 | 🔜 Future | Production-ready v1.0.0 |

Resource-based (CPU/memory) limiting and transparent proxy mode are deliberately
deferred to post-v1.0 — see Future Work in the roadmap for the reasoning.

## Development Commands

```bash
# Build library only (lightweight, no server deps)
cargo build --lib

# Build binary with server features
cargo build --features server

# Run all tests (library + server + doctests)
cargo test --all-features

# Run a specific test / integration tests only
cargo test test_name
cargo test --test '*'

# Code quality (all must pass before milestone completion)
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo audit

# All pre-commit checks at once (or: git config core.hooksPath .git-hooks)
./.git-hooks/pre-commit

# Deterministic multi-node simulator (CSV/JSON artifacts + SVG charts)
cargo run --features sim --example cluster_sim -- --list
cargo run --features sim --example cluster_sim -- --scenario partition --seed 42 --plot
cargo run --features sim --example cluster_sim -- --matrix --seed 42   # benchmark table

# Full scenario matrix + 50-node sweep (slow subset)
cargo test --all-features -- --ignored

# Docs
cargo doc --no-deps --open
```

**Run a local 3-node cluster** (manual verification):
```bash
NENYA_ENABLE_GOSSIP=1 NENYA_LISTEN_ADDR=127.0.0.1:8080 NENYA_GOSSIP_ADDR=127.0.0.1:8081 cargo run --features server &
NENYA_LISTEN_ADDR=127.0.0.1:8090 NENYA_GOSSIP_ADDR=127.0.0.1:8091 NENYA_SEED_NODES=127.0.0.1:8081 cargo run --features server &
NENYA_LISTEN_ADDR=127.0.0.1:8100 NENYA_GOSSIP_ADDR=127.0.0.1:8101 NENYA_SEED_NODES=127.0.0.1:8081 cargo run --features server &

curl -X POST localhost:8080/should_throttle -H 'Content-Type: application/json' -d '{"scope":"test"}'
curl localhost:8090/health   # should report peers
jobs -p | xargs kill
```

## Codebase Structure

```
src/
├── lib.rs              # Library: RateLimiter (token bucket + sliding window + engine)
├── engine/             # Library: RateController trait + PidEngine /
│                       #   BayesianEngine (Kalman) / HybridEngine, staleness curve
├── pid_controller.rs   # Library: PID algorithm (error bias, anti-windup)
├── main.rs             # Binary entry (compile_error! without `server` feature)
├── api/                # HTTP API: handlers, RateLimitManager, metrics, errors
├── config/             # Env-var config (Config::from_env) — TOML is planned, NOT yet implemented
├── gossip/             # Chitchat integration: manager (per-scope keys, compact
│                       #   encoding), sync loop, age-weighted staleness decay
│                       #   (aggregate.rs) and two-tier promotion/demotion policy
│                       #   (tier.rs) — both also compiled under `sim` so the
│                       #   simulator runs real code
├── sim/                # Deterministic multi-node simulator (feature `sim`):
│                       #   virtual clock, message-bus gossip model, seeded
│                       #   workloads, scenario library, metrics/artifacts
└── discovery/          # Placeholder only (Milestone 8)
examples/               # cluster_sim (simulator CLI + SVG charts, feature `sim`),
                        #   request_simulator (terminal demo), cluster_load_generator
tests/                  # Integration tests (HTTP API, multi-node gossip),
                        #   simulation.rs (scenario acceptance thresholds),
                        #   model_checking.rs (stateright), property_sim.rs
nenya-sentinel/         # Deprecation stub only — the binary is now `nenya` itself
```

**Library** (`src/lib.rs`, `src/pid_controller.rs`, always compiled, deps: num-traits + log only):
- `RateLimiter<T>`: token bucket for per-request decisions, sliding window for
  rate measurement, PID adjusts token refill rate. Hot path ~40ns.
- Generic over `T: Float + Signed + FromPrimitive`; builders for both types
- Explicit-timestamp APIs (`should_throttle_at`, `update_state_at`) exist and are
  the hook for deterministic simulation (Milestone 4) — prefer extending these
  over adding internal `Instant::now()` calls
- Distribution hooks: `set_peer_observations()` (per-peer `(id, rate, age)`
  — the engines' input) plus `set_external_accepted_request_rate()` /
  `set_num_peers()` (reporting metrics and legacy input path) and
  `cluster_target()` — this is the entire library-side coordination surface

**Server** (feature `server`):
- `api::RateLimitManager`: tiered scope map (`local` non-distributed /
  `tail` compact equal-share / `hot` full limiter + gossip), scopes
  auto-created via pattern match (exact > most specific wildcard > `*`
  default); distributed scopes start tail and promote per `gossip::tier`
- `gossip::gossip_sync_loop`: every 500ms — aggregate peer observations,
  run tier maintenance (peer-triggered promotion, demotion hysteresis,
  budget eviction, TTL sweep), publish hot-scope keys + per-pattern tail
  aggregates
- Default engine (equal-division PID): each node targets
  `cluster_target / live_nodes` with its *local* accepted rate as the
  feedback signal; peer observations contribute liveness only
- Gossip is enabled by `NENYA_SEED_NODES` being non-empty or `NENYA_ENABLE_GOSSIP=1`

## Key Design Decisions

- **HTTP/JSON API, not gRPC** — simplicity and cross-language support
- **Chitchat for gossip** — anti-entropy (Scuttlebutt) + phi accrual failure
  detection; better state-propagation reliability than SWIM
- **Estimation vs. control separation (Milestone 5, implemented)**: the
  `RateController` trait (`src/engine/`) receives per-peer `(rate, age)`
  observations, not a pre-aggregated sum — aggregation strategy is part of
  what engines compete on. Default engine is `pid`
  (data: `docs/engine-comparison.md`); bayesian/hybrid selectable via
  explicit config only
- **Simulation before tuning**: control-loop changes (gains, engines, gossip
  parameters) must be evaluated in the deterministic simulator (Milestone 4)
  before shipping; don't hand-tune against real clusters
- **Two-tier coordination for per-user scale (Milestone 6, implemented)**:
  gossip only scopes near their limit (promotion at sweep-derived 50%
  estimated cluster utilization via `local_rate × num_peers` over an 8s
  estimator window); the heavy-tailed remainder is enforced locally at
  `limit / num_peers` with compact state (~360 B/scope measured at 1M).
  Per-user throttling at millions of users is the primary use case — never
  assume all scopes gossip
- **Transport-agnostic coordination**: gossip is one transport for the core
  loop (local enforcement + periodic rate sharing + feedback control). The
  serverless analog is a blackboard store (DynamoDB/ElastiCache/Durable
  Objects) synced by the embedded library — same engine, same staleness
  semantics. Keep sync/promotion/decay logic separable from Chitchat
  specifics; gossip inside frozen, inbound-less function runtimes is a
  non-goal (serverless uses service mode today, blackboard later)
- **No invented constants**: tunables like `promote_utilization` ship with
  simulator-derived defaults (published sweep curves), exposed in config —
  never bare magic numbers
- **Upstream quota arbitration is the flagship use case**: `cluster_target`
  can be an externally imposed provider quota (Bedrock TPM, AgentCore TPS,
  third-party API limits); scopes = users; the fleet converges under the cap
  while no user starves the rest. Cost-weighted rates (LLM tokens/sec, not
  requests/sec) land in Milestone 7.3; the AgentCore integration (8.3) is the
  flagship and takes precedence over generic Lambda-protection adapters
  (deferred to Future Work). The soft-limit caveat doesn't apply since the
  upstream enforces the hard cap
- **Fail-open clients**: SDKs will admit on sidecar timeout — the rate limiter
  must never become an availability dependency
- **Security model**: cluster secret authenticates gossip (Milestone 9, not yet
  implemented); discovery is unauthenticated (only finds candidates)
- **No backwards compatibility required**: 0.x, no external users, breaking
  changes are fine

## Testing Philosophy

- Unit tests live in `src/*/tests` modules; integration tests in `tests/`
- Multi-node integration tests spawn real processes; keep them deterministic and
  non-interactive (CI runs them)
- Time-dependent library tests should prefer the explicit-timestamp APIs over
  `sleep()` where possible
- From Milestone 4 onward, cluster dynamics (convergence, overshoot, partitions)
  are tested in the in-process simulator with seeded RNG — same seed must give
  identical results; real-process tests remain as a reality check
- No flaky tests tolerated; fast subset must stay under ~30s

## Milestone Workflow

1. Open `docs/roadmap.md`, find the current milestone and its task checklist
2. Read the referenced architecture sections; plan if non-trivial
3. Implement with tests alongside
4. Verify: `cargo test --all-features && cargo fmt -- --check && cargo clippy --all-targets --all-features -- -D warnings`
5. Commit with the milestone's suggested message (user handles commits unless asked)
6. Check off the milestone in `docs/roadmap.md`, update its "Current Milestone"
   section and the table in this file

## Common Patterns

**Builder pattern** (library):
```rust
let limiter = RateLimiterBuilder::new(100.0)
    .min_rate(50.0)
    .max_rate(200.0)
    .pid_controller(pid)
    .build();
```

**Error handling**: `Result<T, Error>` with custom error types; never panic in
library code. **Logging**: `tracing` macros in server code, `log` in the library.

## References

- **Scuttlebutt**: "Efficient Reconciliation and Flow Control for Anti-Entropy Protocols"
- **Chitchat**: https://quickwit.io/blog/chitchat
- **PID control**: standard control theory; algorithm steps in README.md
- **Kalman filtering** (Milestone 5): Welch & Bishop, "An Introduction to the
  Kalman Filter" — cite authoritative sources for any constants/derivations
