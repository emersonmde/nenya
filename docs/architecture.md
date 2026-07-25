# Nenya Architecture

This document describes the architecture of Nenya, a distributed adaptive rate
limiter using feedback control and gossip-based coordination.

Every section is marked **[Implemented]** or **[Planned — Milestone N]** so it's
always clear what exists versus what is design intent. Update markers as
milestones complete. See [roadmap.md](roadmap.md) for the milestone plan.

## Overview

Nenya is a single crate with two faces:

- **Library** (`nenya`): adaptive rate limiting with PID control; no server or
  network dependencies. **[Implemented]**
- **Binary** (`nenya`, behind the `server` feature): a sidecar that wraps the
  library with an HTTP API and gossip-based cluster coordination.
  **[Implemented]** (the old `nenya-sentinel` crate name is deprecated; a stub
  remains for the crates.io name only)

### Design Goals

1. **No central coordinator**: nodes converge on a cluster-wide limit through
   gossip + local feedback control alone. This is the core differentiator from
   Redis-backed or owner-hashing designs (Gubernator, Kong, Envoy ratelimit).
2. **Minimal configuration**: useful with zero config; configurable when needed
3. **Universal deployment**: Docker, Kubernetes, ECS, VMs, bare metal
4. **Simple integration**: one local HTTP call (or thin SDK) per request
5. **Eventually consistent, honestly**: rate limits are *soft*. Worst-case
   overshoot ≈ gossip propagation delay × excess demand. Nenya is for fairness
   and overload protection, not billing-grade quota enforcement.
6. **Fail gracefully**: partitions degrade toward local/stale-informed decisions
   and recover automatically; the limiter must never become an availability
   dependency

**Flagship use case — upstream quota arbitration**: many APIs sit in front of
a scarce, externally capped resource (Bedrock model TPM, AgentCore
`InvokeAgentRuntime` TPS, any third-party API quota). Set `cluster_target` to
that quota and scopes to users: the fleet collectively converges under the
provider's cap (no surprise upstream 429s, smoothed demand instead of
slamming the ceiling) while no single user starves the rest. The soft-limit
caveat (goal 5) doesn't bite here — the upstream enforces the hard cap; nenya
provides fair division and smoothing, which nothing AWS-native offers
per-user across a fleet. Cost-weighted rates (LLM tokens/sec rather than
requests/sec) are planned — see roadmap Milestone 7.3.

## System Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ Application (any language)                                  │
│   guard clause / SDK: POST localhost:8080/should_throttle   │
└───────────────────────┬─────────────────────────────────────┘
                        ▼
┌─────────────────────────────────────────────────────────────┐
│ Nenya sidecar                                               │
│  HTTP API (axum) ──► RateLimitManager                       │
│                        HashMap<scope, RateLimiter>          │
│                        pattern-matched scope auto-creation  │
│                        each scope: token bucket + sliding   │
│                        window + control engine              │
│  Gossip sync loop (500ms):                                  │
│    publish local per-scope accepted rates                   │
│    aggregate peer rates ──► set_external_accepted_rate()    │
│  Chitchat gossip (UDP) ◄──────────────► [peer nodes]        │
│  Discovery (planned): static / Swarm / K8s / ECS            │
└─────────────────────────────────────────────────────────────┘
```

## Core Rate Limiting (Library) — [Implemented]

Three cooperating mechanisms per limiter (`src/lib.rs`):

1. **Token bucket** — per-request admission decision. Fast (~40ns), immune to
   timestamp collisions.
2. **Sliding window** — measures the *accepted* request rate; this is the
   feedback signal for the controller (not the raw offered rate).
3. **Control engine** — adjusts the token refill rate toward the target.
   Currently PID (`src/pid_controller.rs`): error bias, integral windup
   clamping, anti-windup feedback, output clamping.

**Distribution hooks** (the entire library-side coordination surface):
- `set_external_accepted_request_rate(rate)` — inject the sum of peer rates
- `set_num_peers(n)` — for equal division of the cluster target
- `cluster_target(rate)` — cluster-wide target; each node aims for
  `cluster_target / num_nodes` using `local + external` as its signal

**Explicit-timestamp APIs** (`should_throttle_at`, `update_state_at`) allow the
caller to control time — this is the seam the deterministic simulator (Milestone
4) builds on. Avoid adding internal `Instant::now()` calls on paths the
simulator must drive.

Generic over `T: Float + Signed + FromPrimitive`; builder pattern for both the
limiter and the PID controller.

## HTTP API Server — [Implemented]

**Framework**: axum. **Binding**: `127.0.0.1:8080` by default (localhost only).

```
POST /should_throttle
  Request:  {"scope": "api#key123"}
  Response: {"should_throttle": false, "current_rate": 45.2,
             "target_rate": 100.0, "accepted_rate": 40.1}

GET /health    → {"healthy": true, "peers": 5, "scopes": 12}
GET /metrics   → Prometheus text format
```

An OpenAPI spec and versioning policy (additive-only) land with the SDK
milestone. **[Planned — Milestone 7]**

## Rate Limit Manager — [Implemented]

`api::RateLimitManager`: `HashMap<String, RateLimiter<f64>>` with scope
auto-creation on first use.

**Pattern matching** (`ScopePattern`): glob-style with priority
exact match > most specific pattern > `*` default.

```toml
# Target state; TOML config itself is planned (see Configuration)
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

**Scope cardinality**: every *gossiped* scope is state replicated to every
node, so naive gossip tops out around thousands of scopes. Per-user
cardinality (millions of scopes — the primary use case) is handled by
two-tier coordination: local-share enforcement for the tail, gossip only for
scopes near their limit. See Two-Tier Coordination below.

## Gossip Coordination — [Implemented]

**Library**: [Chitchat](https://quickwit.io/blog/chitchat) — Scuttlebutt
anti-entropy with phi accrual failure detection. Chosen over SWIM-style
libraries because state propagation (not just membership) is the payload, and
anti-entropy doesn't miss updates.

**Gossip state** (actual schema, `src/gossip/state.rs`):

```rust
struct GossipState {
    node_id: String,
    scopes: HashMap<String, ScopeState>,
    timestamp: SystemTime,
}
struct ScopeState {
    accepted_rate: f64,      // the only value coordination needs
    timestamp: SystemTime,   // opaque change marker for age-at-receipt tracking
}
```

**Sync loop** (`src/gossip/sync.rs`, every `sync_interval`, default 500ms):
1. Refresh and publish local per-scope accepted rates
2. Aggregate peer rates per scope with age-weighted staleness decay
   (`src/gossip/aggregate.rs`)
3. `set_external_accepted_request_rate(weighted_sum)` + `set_num_peers(live)`
   on every limiter in one pass; scopes with no live peer data are reset to
   zero so vanished peers leave no phantom load

**Equal division PID**: each node independently computes
`cluster_total = local + sum(peers)` and adjusts its local refill rate so the
cluster converges on `cluster_target`. Conservative default gains
(Kp=0.5, Ki=0.05, Kd=0.05) tolerate 1–2s gossip lag.

### Staleness decay — [Implemented, Milestone 3]

Peer contributions are weighted by freshness: full weight up to
`2 × sync_interval`, linear decay to zero at `stale_timeout` (default 10s,
`NENYA_STALE_TIMEOUT_MS`), dropped past it. Age is **age-at-receipt**: the
`GossipManager` records the local monotonic `Instant` whenever a peer's
gossiped timestamp changes, and never compares peer wall-clock times against
local time — so cross-node clock skew (including future-dated timestamps)
cannot affect decay. A silent peer also stops counting toward `num_peers` at
`stale_timeout`, before Chitchat's failure detector evicts it. The decay and
aggregation logic (`src/gossip/aggregate.rs`) is transport-agnostic — it
consumes `(rate, age)` observations — so the simulator (Milestone 4) and a
future blackboard transport reuse it unchanged.

### Lock behavior — [Measured, Milestone 3]

The sync loop takes the manager write lock twice per tick (local rate refresh,
then a single merged update pass), each a lock-only limiter traversal with
gossip I/O between them. Benchmarked at 1/100/1000 scopes
(`benches/gossip_contention_bench.rs`): decision-path p50/p99 are
indistinguishable with the sync loop idle vs. active (p99 ≤ ~210ns); the only
effect is a rare max-latency outlier (tens of µs) when a decision waits behind
a sync pass. Finer-grained locking (per-limiter, `DashMap`) is deliberately
not pursued — the measurement didn't justify it.

## Coordination Transports — [Gossip: Implemented; Blackboard: Future]

Nenya's core loop is **local in-memory enforcement + periodic asynchronous
sharing of accepted rates + feedback control toward a global target**. Gossip
is one *transport* for the sharing step — the right one for long-lived nodes —
not the idea itself. The engine consumes `(rate, age)` observations without
caring how they arrived, so the sync/promotion/decay logic is kept separable
from Chitchat specifics.

| Deployment | Integration | Transport |
|------------|-------------|-----------|
| EC2 / ECS / K8s / Swarm / Compose / bare metal | Sidecar + SDK | Gossip mesh (Chitchat) **[Implemented]** |
| Anything that can make an HTTP call (simplest serverless path) | SDK → remote service-mode cluster | None (client of a gossip cluster) **[Planned — Milestone 8]** |
| Lambda layers / Cloudflare Workers (embedded) | nenya library in the runtime | Blackboard: DynamoDB / ElastiCache / Durable Objects **[Future — post-v1.0]** |

**Blackboard transport**: same sync loop, hub-shaped — each warm instance
writes its hot-scope rates to a shared store (e.g., a DynamoDB item per scope
holding an instance map with TTLs) and reads the aggregate each interval.
Staleness semantics transfer unchanged: a frozen Lambda environment ages out
exactly like a partitioned gossip peer. Decisions stay local and in-memory;
the store is touched once per sync interval per instance, never per request —
the defining difference from typical hand-rolled DynamoDB rate limiters.
Two-tier promotion is load-bearing here (only hot scopes touch the store,
bounding cost). Tradeoff: ElastiCache is cheap per-op but needs VPC + cluster
(ops burden ≈ service mode); DynamoDB is serverless-native but pay-per-op,
so sync intervals stretch to 2–5s.

## Two-Tier Coordination for Per-User Scale — [Planned — Milestone 6]

Per-user distributed throttling at large cardinality (10⁵–10⁶+ users) cannot
gossip every scope. The design exploits two facts: load balancers spread a
user's traffic roughly uniformly (so `local_rate × num_nodes` is a good local
estimate of that user's cluster rate), and usage is heavy-tailed — only a small
fraction of users are near their limit at any instant.

- **Tail tier (default)**: local-only enforcement of the equal share
  `limit / num_peers`; compact state (token bucket only, no engine), no gossip
- **Hot tier**: full gossip coordination, entered when estimated cluster
  utilization crosses `promote_utilization` (per-pattern config; shipped
  default derived from a simulator sweep — knee of promoted-set-size vs.
  worst-case-overage — not hand-picked); demotion below `demote_utilization`
  with hysteresis; hard per-node budget K on gossiped scopes
  (lowest-utilization evicted, logged — no silent truncation)
- **Tail visibility**: one aggregate tail rate per pattern per node keeps
  service-level totals accurate; a fixed-size mergeable count-min sketch of
  tail rates is a candidate if per-user tail estimates prove necessary
  (one-sided error → throttles conservatively)
- **Error bound**: an unpromoted user can exceed their limit only via routing
  skew or promotion lag — and skew itself triggers promotion on the hot node,
  so exposure is transient. Bounds are quantified in the simulator
  (Pareto-traffic and sticky-routing scenarios) and documented, not assumed.

## Deterministic Simulator — [Implemented, Milestone 4]

The primary tool for correctness testing, benchmarking, and control-loop
experimentation (`src/sim/`, feature `sim`, no added dependencies). Real
multi-process tests are too slow and nondeterministic to sweep parameters;
the interesting dynamics (gossip lag → phase lag → oscillation, partition
overshoot, convergence after churn) need N nodes and a network model.

- **Virtual clock**: `base Instant + tick × index` (10ms default tick)
  driving the library's explicit-timestamp APIs; no wall-clock sleeps — a
  60s scenario runs in milliseconds
- **Simulated cluster** (`sim::cluster`): N in-process nodes with real
  `RateLimiter`s constructed exactly as `RateLimitManager` builds them;
  message-bus gossip with configurable delay, jitter, loss, and partitions;
  each node runs the `gossip_sync_loop` sequence against the *real*
  `gossip::aggregate` decay code (compiled under both `server` and `sim`)
- **Seeded workloads** (`sim::workload`): constant/step/ramp/burst/sinusoidal
  patterns × deterministic or Poisson arrivals, per-node skew weights. RNG is
  an in-repo SplitMix64 (Vigna's public-domain reference), so the stream can
  never shift under a dependency bump — same seed → byte-identical CSV/JSON
- **Scenario library** (`sim::scenario`): steady below/at/above, step, ramp,
  burst, join, leave, partition+heal, skew, sinusoidal, scale 2/5/10/50
- **Metrics** (`sim::metrics`): max + integrated overshoot, per-event
  convergence time (±5% band, 5s hold, 2s smoothing), steady-state
  oscillation, fairness CV, integrated undershoot
- **Artifacts**: CSV/JSON time series plus static SVG charts
  (`examples/cluster_sim.rs`, plotters SVG backend only); the egui realtime
  dashboard examples and their dependency tree are removed
- **Benchmark harness**: `cluster_sim --matrix` emits a markdown comparison
  table across all scenarios; gains and the anti-windup clamp are
  overridable per run — this is the A/B tool for Milestone 5 engines
- **CI** (`tests/simulation.rs`): scenario acceptance thresholds as tests
  (fast subset ~0.1s); full matrix + 50-node sweep behind `--ignored`.
  `tests/model_checking.rs` (stateright) exhaustively verifies the
  aggregation bookkeeping invariants (no phantom load past `stale_timeout`,
  each peer counted exactly once with exactly its decay weight, correct
  live-peer count after quiescence) over all message interleavings of a
  small cluster; `tests/property_sim.rs` (proptest) covers decay-weight and
  token-bucket invariants over arbitrary inputs

**First finding**: the scenario matrix exposed unbounded PID integral windup
in the production limiter defaults — a partitioned minority (fair share above
its offered load) wound up its integral term and overshot for ~60s after
heal. Production scope limiters now ship with an anti-windup clamp
(`error_limit = 0.2 × target`, `ScopePattern::get_error_limit`), the 0.2
derived from a simulator sweep of {0.1, 0.2, 0.5}: post-heal re-convergence
dropped to ~5s with marginal trade-offs between the three values.

## Control Engines — [Planned — Milestone 5]

The controller becomes swappable behind a narrow trait:

```rust
trait RateController {
    /// observations: per-peer (accepted_rate, age) — NOT a pre-aggregated sum.
    /// Aggregation strategy is part of what engines compete on.
    fn update(&mut self, local_rate: f64, observations: &[(f64, Duration)],
              num_peers: usize, cluster_target: f64, dt: Duration) -> f64;
}
```

Three candidate engines, all first-class:

- **PidEngine**: the existing controller ported unchanged
- **BayesianEngine** (estimate-and-set): each peer's true rate is a latent
  variable observed through delayed gossip samples. Scalar Kalman filter per
  (peer, scope); process noise grows variance between samples, so
  **staleness = uncertainty** (subsuming the Milestone 3 decay heuristic).
  Admission is computed against an upper confidence bound of the cluster-rate
  estimate: automatically conservative during partitions/churn, aggressive when
  the estimate is tight.
- **HybridEngine**: Kalman-filtered estimate feeding PID. The separation
  principle would make this provably optimal for a linear-Gaussian, delay-free
  plant — assumptions gossip coordination violates (variable delay, clamping
  nonlinearity, non-Gaussian noise, churn), and LQG-style designs carry no
  guaranteed stability margins even when they hold. So it is a benchmark
  candidate like the others, not a presumed winner.

The engine is an **explicit config option** (`engine = "pid" | "bayesian" |
"hybrid"` per scope) — never selected at runtime. The Milestone 4 benchmark
matrix (convergence, overshoot, oscillation, partition behavior, parameter
sensitivity) decides only which value ships as the documented default,
recorded in `docs/engine-comparison.md`. Engines run in the sync loop (per
second per scope), never on the per-request hot path.

## Configuration

**Current state [Implemented]**: environment variables only
(`Config::from_env`, `src/config/mod.rs`):

```bash
NENYA_LISTEN_ADDR=127.0.0.1:8080      # client API
NENYA_GOSSIP_ADDR=0.0.0.0:8081        # gossip transport
NENYA_SEED_NODES=host1:8081,host2:8081
NENYA_ENABLE_GOSSIP=1                 # gossip also enabled if seed nodes set
NENYA_DEFAULT_TARGET_RATE=100.0       # plus default min/max rate, kp/ki/kd
NENYA_SYNC_INTERVAL_MS=500            # gossip sync loop interval
NENYA_STALE_TIMEOUT_MS=10000          # silent-peer decay horizon; must exceed
                                      #   2 × sync interval
```

**Target state [Planned — Milestones 7-9]**: layered hierarchy — hardcoded
defaults → TOML file (`./nenya.toml`, `/etc/nenya/nenya.toml`, `NENYA_CONFIG`)
→ env overrides → CLI flags. TOML adds `[[rate_limits]]` pattern tables with
per-scope engine selection and parameters, `[discovery]`, `[gossip]`, and
cluster secret loading. Zero-config platform auto-detection (K8s service
account → kubernetes discovery; ECS metadata endpoint → ecs; else static) lands
with Milestone 8.

## Discovery Layer — [Planned — Milestone 8]

`src/discovery/` is a placeholder today; the binary uses static seed nodes from
env vars.

```rust
#[async_trait]
trait PeerDiscovery: Send + Sync {
    async fn discover_seeds(&self) -> Result<Vec<SocketAddr>>;
}
```

**Design principle**: gossip needs only *one* live seed — membership
propagation enumerates the rest. Discovery finds candidates, not topology.
With dynamic discovery (DNS over live members, tag queries), "seed" is not a
role: every live node is a seed, so there is no fixed node whose downtime
blocks autoscale joins. Providers chain: configured providers run in order,
results union and dedupe; if none yields a seed, start standalone with a
prominent warning.

**Bootstrap is symmetric and continuous**: all nodes ship identical config
(self-address filtered from resolved seeds); there is no ordered first-node
setup. Discovery re-resolution and join retry run for the life of the process,
so cold-start singleton clusters (DNS registration lag) merge automatically,
and discovery outages never affect already-joined members.

- **Tier 1 — generic DNS seed resolution** (the universal path): resolve one
  configured name to A/AAAA records. Covers K8s headless services, Swarm
  `tasks.<service>`, Compose service names, ECS Cloud Map, and Route53/custom
  DNS over EC2 or bare-metal nodes — one code path
- **Tier 2 — static seed list** [effectively implemented via env]: always
  available as the fallback
- **Tier 3 — optional platform providers**: Kubernetes API (label selectors),
  EC2 instance tags (the Consul/Nomad "cloud auto-join" pattern — covers
  standalone EC2 with no DNS setup), Docker API if Swarm DNS proves
  insufficient
- **mDNS — opt-in only, never an automatic fallback**: multicast doesn't work
  in most cloud networks (AWS VPCs don't forward it; most K8s CNIs and Docker
  overlay networks drop it), so as a fallback it would fail silently exactly
  where people deploy. Explicitly configured, it's a good zero-config option
  for flat L2 networks (bare metal, labs)
- Environment auto-detection sets *defaults* (K8s service account → headless
  DNS; ECS metadata endpoint → Cloud Map; else static/DNS from config)
- Periodic re-resolution so scale-ups are found without restart

Discovery is unauthenticated — it only finds *candidates*. Trust is established
at gossip join (see Security).

## Client SDKs — [Planned — Milestone 7]

Thin (~100-line) clients for Rust, Python, Node/TypeScript, and Go:
one call + one middleware/decorator per language, in `sdks/<language>/`.

**Endpoint**: localhost sidecar by default; configurable to a remote nenya
service URL ("service mode") for callers that can't host a sidecar — see
Serverless under Deployment Patterns. Token leasing (batched allowances
decremented locally, refreshed asynchronously) is the planned post-v1.0
optimization that makes service mode near-zero-overhead per call.

**Failure policy**: fail-open with a short timeout (default ~5ms). If the
sidecar is down or slow, admit the request — a rate limiter must not become an
availability dependency. A shared JSON conformance spec runs every SDK against
a real sidecar in CI.

Transparent proxy mode (Envoy `ext_authz`) is post-v1.0 future work.

## Security Model — [Planned — Milestone 9]

Nothing is implemented yet; today any node that can reach the gossip port can
participate.

- **Cluster secret** loaded from file (`/run/secrets/nenya_cluster_secret`),
  env (`NENYA_CLUSTER_SECRET`), or TOML; startup error if absent (once enforced)
- **Message authentication**: HMAC over gossip payloads; tampered or
  unauthenticated messages rejected (likely a Chitchat transport wrapper)
- **Join handshake**: challenge-response so wrong-secret nodes never enter the
  peer set
- **Optional mTLS** transport wrapper

## Observability

**Metrics [Implemented]** at `GET /metrics` (Prometheus): request counters by
scope/throttled, current/target/accepted rate gauges, peer and scope gauges.

**Tracing [Implemented]**: `tracing` with env-filter (`RUST_LOG`). OTLP export
is future work.

## Failure Modes

### Network partition
Each side keeps operating on local + last-known peer rates. With Milestone 3
decay, stale peer contributions fade to zero within `stale_timeout`, after
which each side controls against what it can see — the cluster may transiently
admit up to ~2× target across both sides of a clean split (this is the soft-limit
tradeoff, by design). Re-convergence after heal is bounded by gossip propagation
plus controller convergence time, and is a required simulator scenario.

### Node failure
Phi accrual marks the node dead; Chitchat evicts it; decay (Milestone 3)
removes its rate contribution within `stale_timeout` even before eviction.
Remaining nodes' equal-division share grows (`num_peers` drops) and controllers
re-converge.

### Rapid scaling
New nodes join via any seed, receive full state via anti-entropy, and
participate immediately; convergence is O(log N) gossip rounds.

### Sidecar failure (application view)
SDKs fail open on timeout. The application keeps serving; the cluster briefly
loses one node's contribution to coordination — equivalent to a node failure.

## Performance Characteristics

Measured where noted; otherwise targets.

- Library decision hot path: ~40ns (measured, Milestone 0 baseline)
- PID computation: ~1-2ns (measured)
- `should_throttle` HTTP round trip: <1ms target (localhost)
- Gossip propagation: ~0.5-2s (500ms sync interval + gossip rounds)
- Throughput target: >50K RPS per node (HTTP parsing bound, not limiter bound)
- Memory: ~1KB per scope target (sliding window + controller state)
- Gossip bandwidth: O(scopes) per node state, anti-entropy deltas via Chitchat

## Testing Strategy

- **Unit**: library algorithms, pattern matching, config, gossip state
  serialization, aggregation/decay logic
- **Simulation [Implemented — Milestone 4]**: all cluster dynamics
  (convergence, overshoot, oscillation, partitions, churn) as deterministic
  seeded scenarios with CI acceptance thresholds (`tests/simulation.rs`)
- **Model checking & properties [Implemented — Milestone 4]**: `proptest` for
  library invariants; `stateright` (Rust model checker, runs against the real
  aggregation code) to exhaustively verify discrete safety invariants over all
  message interleavings — no phantom load past `stale_timeout`, no
  self/double-counting, correct peer accounting. Quantitative dynamics stay in
  the simulator (model checkers can't express them); TLA+ is reserved for the
  Milestone 9 auth handshake
- **Integration**: real-process multi-node tests as a reality check on the
  simulator's gossip model; HTTP API tests with reqwest
- **Load/stress/soak [Milestone 10]**: wrk2 profiles, 10K-scope stress, 24h soak
- All CI-run tests deterministic, fast subset <30s

## Deployment Patterns — [Planned — Milestone 8]

Target: official multi-arch Docker image + one tested, copy-paste block per
platform in `deploy/`:

- **Docker Compose / Swarm**: one service block; Swarm DNS discovery
- **Kubernetes**: native sidecar (initContainer with `restartPolicy: Always`,
  K8s ≥1.28) + headless service for discovery
- **AWS ECS**: task definition with container dependency ordering, Cloud Map
  discovery
- **VMs/bare metal**: systemd unit + static seeds
- **Serverless (AWS Lambda etc.) — service mode**: gossip *inside* function
  environments is a non-goal (frozen between invocations, no inbound
  connections, extreme churn). Instead the same binary runs as a small
  standalone regional cluster (e.g., 3 Fargate tasks behind an NLB) and
  functions call it through the SDK's remote endpoint — identical decorator,
  different URL. Fail-open semantics are load-bearing here. The priority
  serverless integration is **AgentCore quota arbitration** (roadmap 8.3):
  AgentCore Runtime agents run in session-scoped microVMs with the same
  constraints as Lambda, so agents and Lambda callers alike use the SDK guard
  against service mode. Protecting the Lambda platform itself via earlier
  interception (API Gateway authorizer, layer early-return — the interception
  ladder) is deliberately deferred; ladder details in roadmap Future Work.
  Post-v1.0: token leasing + a Lambda Extension lease cache cut per-invoke
  overhead to near zero, and the embedded blackboard transport (see
  Coordination Transports) removes the service entirely for teams willing to
  provision a table

## Future Work (Post-v1.0)

Tracked in [roadmap.md](roadmap.md) Future Work — highlights: resource-based
(CPU/memory) limiting as a concurrency-control sibling engine, transparent
proxy mode, serverless token leasing + Lambda Extension, adaptive engine
tuning, hierarchical/shared capacity pools, priority weighting, dynamic
reconfiguration, state persistence, multi-cluster namespacing.
