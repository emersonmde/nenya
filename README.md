[![Rust Build](https://github.com/emersonmde/nenya/actions/workflows/rust.yml/badge.svg)](https://github.com/emersonmde/nenya/actions/workflows/rust.yml)
[![Docs](https://img.shields.io/docsrs/nenya/latest)](https://docs.rs/nenya)
[![crates](https://img.shields.io/crates/v/nenya.svg)](https://crates.io/crates/nenya)
[![License](https://img.shields.io/crates/l/nenya.svg)](LICENSE)

# Nenya

**Distributed, adaptive rate limiting with no central coordinator.** Nodes
share only per-scope accepted rates over gossip; a local control loop on
each node converges the fleet onto a cluster-wide limit. No Redis, no
owner-hashing, no rate-limit service to operate — add a node and the fleet
re-divides the limit by itself.

The design target is **per-user limits at per-user scale**: millions of
scopes per cluster, where a user's traffic is spread across nodes by a load
balancer and one noisy user must not exhaust a shared limit — for example,
fairly dividing an upstream provider quota (Bedrock TPM, a partner API's
rate cap) across a fleet and its users.

![Offered load doubles from 300 to 600 rps at t=30s; the cluster's
accepted rate holds at the 300 rps target throughout, with one brief
transient at the step](docs/images/step_pid_seed42.svg)

*Offered load (top line) doubles at t=30s; the three-node cluster holds
accepted throughput on the 300 rps target with no coordinator. Sample
jitter is Poisson arrival noise at 500 ms sampling; steady mean is 300.0
before and 302.3 after the step. Deterministic simulator, seed 42 —
reproduce with
`cargo run --features sim --example cluster_sim -- --scenario step --seed 42 --plot`.*

## Why nenya

- **No coordination service.** Gossip (Chitchat/Scuttlebutt) carries a few
  bytes per active scope per second; every enforcement decision is a local
  in-memory token-bucket check (~30 ns). The limiter can never become an
  availability dependency on your request path.
- **Per-user limits that stay cheap at fleet scale.** Every user gets a
  real cluster-wide limit, not a per-node approximation: single-node
  users are capped exactly by a local bucket, and users spread across
  nodes are detected through gossiped rate evidence and coordinated
  before exceeding their limit. Coordination cost scales with the number
  of users *near their limits* — not with population, node count, or
  request rate. Measurements (including the memory and wire footprint at
  10⁶ scopes, and the ablation against gossiping everything) are in
  [docs/capacity-model.md](docs/capacity-model.md).
- **Control theory instead of quota slicing.** A pluggable controller
  (PID by default; Bayesian per-peer Kalman estimation and a hybrid,
  benchmarked in [docs/engine-comparison.md](docs/engine-comparison.md))
  adapts each node's admission rate to what the cluster is actually
  accepting — fleet convergence is ~4 s and flat from 10 to 100 nodes.
- **Every default is derived.** Gains, tier thresholds, estimator windows,
  and TTLs come from published simulator sweeps, with the tables and
  re-run commands in the docs — not hand-picked constants.
- **Honest limits.** Gossip-based limits are *soft*: worst-case overshoot
  ≈ coordination lag × excess demand, and the measured bounds are
  documented (an uncoordinated scope is bounded at 1.25× its limit). If
  you need billing-grade enforcement, put the hard counter at the
  resource; nenya's job is fairness and overload protection under it.

## Quick start

**Library** (no server dependencies):

```toml
[dependencies]
nenya = "0.1"
```

```rust
use nenya::RateLimiterBuilder;
use nenya::pid_controller::PIDControllerBuilder;
use std::time::Duration;

fn main() {
    let pid = PIDControllerBuilder::new(100.0).kp(0.5).ki(0.02).kd(0.08).build();
    let mut limiter = RateLimiterBuilder::new(100.0) // 100 requests/second
        .min_rate(50.0)
        .max_rate(200.0)
        .pid_controller(pid)
        .update_interval(Duration::from_secs(1))
        .build();

    if limiter.should_throttle() {
        // reject or queue the request
    }
}
```

**Sidecar** (distributed):

```bash
cargo install nenya

# First node
NENYA_CLUSTER_SECRET=secret NENYA_ENABLE_GOSSIP=1 nenya

# Every additional node: point at any existing gossip address
NENYA_CLUSTER_SECRET=secret NENYA_SEED_NODES=10.0.0.1:8081 nenya
```

Your service makes one local call per request:

```text
POST localhost:8080/should_throttle   {"scope": "user:1234"}
→ {"should_throttle": false, ...}
```

Configuration is a handful of env vars derived from things you already
know — your limits, how long users stay active, your seed addresses. Node
count and load-balancer policy need no compensation (measured across
uniform, round-robin, least-loaded, and sticky routing). See
[docs/tuning.md](docs/tuning.md).

## How it works

Each scope (user, API key, route) gets a **token bucket** for per-request
decisions, a **sliding window** measuring its accepted rate, and — when
coordination is warranted — a **control engine** that adjusts the bucket's
refill rate.

**Cluster coordination**: every 500 ms, nodes gossip their per-scope
accepted rates. Each node's controller targets its share of the cluster
limit using its local rate as the feedback signal; silent peers decay on a
staleness curve, so crashes and partitions re-divide the limit
automatically within seconds.

**Per-user scale (two-tier, evidence-based)**: most scopes never need
coordination. A scope starts as a compact local bucket enforcing its
**full limit** — a single-node user cannot exceed the limit through one
bucket, so nothing is throttled below the limit on an assumption about
routing. Locally-warm scopes publish their rates; full coordination
engages only when local + peer-observed rates approach the limit *with
nonzero peer evidence*. The result: sticky-session users are served at
~0.98 of offered with zero coordination traffic, spread users are
coordinated before exceeding the limit, and idle users age out after a
TTL.

## Evidence

Control behavior is developed and verified in a **deterministic
multi-node simulator** (`--features sim`): real limiter code, a
message-bus gossip model with delay/jitter/loss/partitions, seeded
workloads (including Zipf populations of 10⁵–10⁶ users), and a virtual
clock — a 60-second scenario runs in milliseconds and the same seed is
byte-identical.

```bash
cargo run --features sim --example cluster_sim -- --list             # scenarios
cargo run --features sim --example cluster_sim -- --matrix --seed 42 # engine benchmark
```

CI asserts the scenario acceptance thresholds, property tests (proptest),
and exhaustive model checking (stateright) of the aggregation and tier
state machines on every commit. The wire format is verified against real
UDP chitchat at 10k scopes.

- [docs/capacity-model.md](docs/capacity-model.md) — scaling ceilings,
  sweep tables behind every default, memory/wire measurements
- [docs/engine-comparison.md](docs/engine-comparison.md) — PID vs
  Bayesian vs hybrid across the scenario matrix
- [docs/architecture.md](docs/architecture.md) — design, each section
  marked Implemented or Planned
- [docs/tuning.md](docs/tuning.md) — operator-facing configuration guide
- [docs/roadmap.md](docs/roadmap.md) — milestone plan and history

**Status:** milestones 0–6 complete (HTTP API, gossip coordination,
simulator, pluggable engines, per-user scale). Next: client SDKs
(Rust/Python/Node/Go) with fail-open semantics.

## Development

```bash
git config core.hooksPath .git-hooks   # tests, clippy, fmt, audit pre-commit
cargo test --all-features
```

## License

MIT — see [LICENSE](LICENSE).
