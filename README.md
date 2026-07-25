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

It ships as one crate with two faces: a dependency-light Rust **library**
for embedded rate limiting, and a **sidecar binary** any language can call
over local HTTP. The problem it is built for is per-user fairness under a
shared limit — one noisy user must not exhaust a quota everyone depends
on, such as an upstream provider's rate cap (Bedrock TPM, a partner API)
divided across a fleet and its users.

![Offered load doubles from 300 to 600 rps at t=30s; the cluster's
accepted rate holds at the 300 rps target throughout, with one brief
transient at the step](docs/images/step_pid_seed42.svg)

*Offered load (top line) doubles at t=30s; the three-node cluster holds
accepted throughput on the 300 rps target. Jitter is Poisson arrival noise
at 500 ms sampling; steady mean is 300.0 before and 302.3 after the step.
Deterministic simulator, seed 42 — reproduce with
`cargo run --features sim --example cluster_sim -- --scenario step --seed 42 --plot`.*

## Why nenya

- **No coordination service.** Every admission decision is a local
  in-memory token-bucket check (~30 ns) — no remote call to a limiter
  backend that can fail, saturate, or add tail latency. Gossip runs off
  the request path and carries tens of bytes per warm scope per second.
- **Per-user limits that stay cheap at fleet scale.** Each user gets a
  real cluster-wide limit, not a per-node approximation. Coordination
  cost scales with the number of users *near their limits* — not with
  population, node count, or request rate; everyone else is enforced by
  a compact local bucket.
- **Control theory instead of quota slicing.** A feedback controller
  (PID by default; a Bayesian estimator and a hybrid are benchmarked
  alternatives) adapts each node's admission rate to what the cluster is
  actually accepting. Fleet convergence is ~4 s, flat from 10 to 100
  nodes.
- **Every default is derived.** Gains, tier thresholds, estimator
  windows, and TTLs come from published simulator sweeps with re-run
  commands — no hand-picked constants.
- **Honest limits.** Gossip-based limits are *soft*: worst-case overshoot
  ≈ coordination lag × excess demand, and the bounds are measured and
  documented rather than assumed.

**Good fit**: per-user or per-API-key fairness across a fleet; staying
under an upstream quota that enforces its own hard cap; overload
protection where a briefly soft limit is acceptable and availability is
not negotiable.

**Not a fit**: billing-grade or security-critical quota enforcement (put
a hard counter at the resource); aggregate DDoS protection by itself
(per-user limits admit `users × limit` in total — pair them with a
coarser service-level scope as the backstop).

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

**Sidecar** (distributed; any language):

```bash
cargo install nenya

# First node
NENYA_CLUSTER_SECRET=secret NENYA_ENABLE_GOSSIP=1 nenya

# Every additional node: point at any existing gossip address
NENYA_CLUSTER_SECRET=secret NENYA_SEED_NODES=10.0.0.1:8081 nenya
```

(`NENYA_CLUSTER_SECRET` is reserved for cluster authentication, which is
on the roadmap but **not yet enforced** — run gossip on a trusted network.)

Your service makes one local call per request; scopes are created on
first use:

```text
POST /should_throttle   {"scope": "user:1234"} → {"should_throttle": false, ...}
GET  /scope_stats?scope=user:1234              # rates and tier, no side effects
GET  /health                                   # liveness + peer count
GET  /metrics                                  # Prometheus text format
```

## Configuration

Set limits from things you already know; the control internals are not
knobs you need to touch (their derivations live in
[docs/capacity-model.md](docs/capacity-model.md)).

| Variable | Meaning | Default |
|---|---|---|
| `NENYA_DEFAULT_TARGET_RATE` | Cluster-wide limit per scope (rps) | 100 |
| `NENYA_SEED_NODES` | Any existing nodes' gossip addresses | — |
| `NENYA_LISTEN_ADDR` / `NENYA_GOSSIP_ADDR` | HTTP / gossip bind addresses | `127.0.0.1:8080` / `0.0.0.0:8081` |
| `NENYA_SCOPE_TTL_SECS` | Idle time before a scope's state is dropped; memory ≈ 360 B × scopes active within this window | 60 |
| `NENYA_SYNC_INTERVAL_MS` | Gossip exchange interval | 500 |

Node count and load-balancer policy need no compensation — measured
identical under uniform, round-robin, least-loaded, and sticky routing.
The full guide, including what to think about for spiky traffic and tight
limits, is [docs/tuning.md](docs/tuning.md).

## How it works

Each scope (user, API key, route — chosen by glob patterns) gets a
**token bucket** for per-request decisions, a **sliding window** measuring
its accepted rate, and — when coordination is warranted — a **control
engine** that adjusts the bucket's refill rate.

**Cluster coordination**: every 500 ms, nodes gossip accepted rates for
the scopes that need coordination. Each node's controller targets its
share of the cluster limit using its local rate as the feedback signal;
silent peers decay on a staleness curve, so crashes and partitions
re-divide the limit automatically within seconds.

**Per-user scale (two-tier, evidence-based)**: most scopes never need
coordination. A scope starts as a compact local bucket enforcing its
**full limit** — a user served by a single node cannot exceed the limit
through one bucket, so nothing is throttled below the limit on an
assumption about routing. Locally-warm scopes publish their rates; full
coordination engages only when local plus peer-observed rates approach
the limit *with nonzero peer evidence*. Simulator-measured results:
sticky-session users are served at ~0.98 of offered with zero
coordination traffic; a user spread across nodes is brought under
coordination within ~1–2 s of approaching the limit (bounded at
1.25 × limit while uncoordinated); idle users age out after a TTL.

## Evidence

Control behavior is developed and verified in a **deterministic
multi-node simulator** (`--features sim`): real limiter code, a
message-bus gossip model with delay/jitter/loss/partitions, seeded
workloads including Zipf user populations, and a virtual clock — a
60-second scenario runs in milliseconds and the same seed is
byte-identical.

```bash
cargo run --features sim --example cluster_sim -- --list             # scenarios
cargo run --features sim --example cluster_sim -- --matrix --seed 42 # engine benchmark
```

CI runs the scenario acceptance thresholds, property tests (proptest),
and exhaustive model checking (stateright) of the aggregation and tier
state machines on every commit; the wire format is verified against real
UDP gossip at 10k scopes.

- [docs/capacity-model.md](docs/capacity-model.md) — scaling ceilings,
  sweep tables behind every default, memory/wire measurements
- [docs/engine-comparison.md](docs/engine-comparison.md) — PID vs
  Bayesian vs hybrid across the scenario matrix
- [docs/architecture.md](docs/architecture.md) — design, each section
  marked Implemented or Planned
- [docs/tuning.md](docs/tuning.md) — operator-facing configuration guide
- [docs/roadmap.md](docs/roadmap.md) — milestone plan and history

**Status:** 0.x, active development. Milestones 0–6 complete (HTTP API,
gossip coordination, simulator, pluggable engines, per-user scale). Next:
client SDKs (Rust/Python/Node/Go) with fail-open semantics, then platform
deployment guides. Breaking changes are still possible before 1.0.

## Development

```bash
git config core.hooksPath .git-hooks   # tests, clippy, fmt, audit pre-commit
cargo test --all-features
```

## License

MIT — see [LICENSE](LICENSE).
