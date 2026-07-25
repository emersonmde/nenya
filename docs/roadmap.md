# Nenya Development Roadmap

This document outlines the implementation plan for Nenya distributed rate limiting, broken into milestones with specific tasks.

## Current Milestone

**Status**: Milestones 0-6 Complete - Ready for Milestone 7 (Client SDKs & API Stabilization)

**Completed**:
- ✅ Milestone 0: Single-crate restructure, HTTP stack, distributed coordination foundation
- ✅ Milestone 1: Single-node HTTP rate limiter with scope management
- ✅ Milestone 2: Distributed gossip coordination with equal division PID
- ✅ Milestone 3: Gossip correctness fixes (stale peer decay, sync loop locking)
- ✅ Milestone 4: Deterministic multi-node simulator, scenario/benchmark suite,
  property tests + stateright model checking (plus first simulator-derived
  production fix: PID anti-windup clamp)
- ✅ Milestone 5: Pluggable control engines (PID / Bayesian-Kalman / hybrid)
  behind the `RateController` trait, engine benchmark + comparison doc, and
  three simulator-derived control fixes (adaptive-window rate estimator,
  adaptive burst allowance — which flattened the fleet-convergence law —
  and a documented negative result on gain scheduling)
- ✅ Milestone 6: Two-tier coordination for per-user scale — compact tail
  tier (~360 B/scope at 1M scopes) with local equal-share enforcement,
  sweep-derived promotion/demotion policy, per-scope gossip keys with
  compact encoding (real-UDP verified at 10k scopes), per-pattern tail
  aggregates, idle-scope TTL eviction, and model-checked tier state
  machine; count-min sketch evaluated and rejected from data

See Milestone 7 below for next steps.

## Principles

- **Iterative development**: Each milestone produces a working, testable system
- **Test-driven**: Write tests alongside implementation
- **Simulation before tuning**: Control-loop changes (PID gains, new engines, gossip
  parameters) must be evaluated in the deterministic simulator before shipping
- **No regressions**: All tests must pass before completing milestone
- **Commit at milestone completion**: Push working code at the end of each milestone

## Workflow

1. Identify current milestone from this roadmap
2. Develop implementation plan (can use plan mode)
3. Implement tasks with tests
4. Verify all tests pass: `cargo test && cargo fmt --check && cargo clippy`
5. Push commit(s) at milestone completion
6. Check off milestone in this file, update "Current Milestone" section
7. Move to next milestone

---

## Milestone 0: Preparation & Cleanup

- [x] **MILESTONE COMPLETE** (commit f0b2889)

Restructured as single-crate (library + optional `server` binary), removed gRPC,
added HTTP stack (axum, tokio, serde), observability (tracing), and the token
bucket + PID hybrid rate limiter with external rate injection.

**Performance baseline** (library, verified):
- Hot path decision: ~40ns (target: <1μs)
- PID computation: ~1-2ns (target: <100ns)
- Throughput: 25M decisions/sec single-threaded

---

## Milestone 1: Single-Node Foundation

- [x] **MILESTONE COMPLETE**

Working single-node HTTP rate limiter:
- TOML config with env overrides, pattern-based `[[rate_limits]]` scopes
- `POST /should_throttle`, `GET /health`, `GET /metrics` (Prometheus)
- `RateLimitManager` with scope auto-creation and pattern priority
  (exact > most specific wildcard > default)
- Tracing + metrics instrumentation

---

## Milestone 2: Gossip Integration

- [x] **MILESTONE COMPLETE** (commit f8e5f74)

Distributed coordination via Chitchat gossip with **equal division PID**:
- Gossip shares per-scope `accepted_rate` (a single f64 per scope per node)
- Each node computes `cluster_total = local + sum(peers)`, targets
  `cluster_target / num_nodes`, and adjusts its local refill rate via PID
- 500ms sync loop: publish local rates → aggregate peer rates →
  `set_external_accepted_request_rate()` + `set_num_peers()`
- Conservative PID defaults (Kp=0.5, Ki=0.05, Kd=0.05) safe for 1-2s gossip lag
- Multi-node integration tests (spawn real processes)

---

## Milestone 3: Gossip Correctness Fixes

- [x] **MILESTONE COMPLETE** (commit 3002913)

**Goal**: Fix known correctness gaps in the gossip aggregation path before building
on top of it.

**Architecture Reference**: [docs/architecture.md](architecture.md) - Gossip Protocol,
Failure Modes

### Tasks

#### 3.1 Stale Peer Decay

The sync loop (`src/gossip/sync.rs`) currently sums whatever peer states it has,
with no regard for age. A peer that goes silent (crash, partition) keeps
contributing its last known rate indefinitely, suppressing local admission with
phantom load. `ScopeState.timestamp` is already gossiped but unused — use it.

- [x] **Age-weighted aggregation** (`src/gossip/aggregate.rs`)
  - Full weight for states fresher than `2 × sync_interval`; linear decay to
    zero at `stale_timeout` (default 10s, `NENYA_STALE_TIMEOUT_MS`); peers past
    `stale_timeout` are dropped and excluded from the live peer count
  - Scopes with no live peer data get their external rate explicitly reset to
    zero each tick (previously a vanished peer's last injected rate persisted
    in the limiter forever)
- [x] **Clock skew handling** — solved via age-at-receipt tracking
  - `GossipManager` records the local `Instant` whenever a peer's gossiped
    timestamp *changes*; age is measured entirely on the local monotonic clock
  - Peer `SystemTime` values are used only as opaque change markers (equality
    comparison), so skewed or future-dated timestamps cannot produce negative
    ages or amplified rates — no clamping needed
- [x] **Verify self-exclusion and dead-node removal**
  - `get_peer_states()` iterates Chitchat's live nodes and always skips self
  - Receipt records are pruned when Chitchat evicts a node; `num_peers` now
    counts only peers with staleness weight > 0 (a silent peer stops counting
    at `stale_timeout`, before Chitchat eviction)

**Tests**:
- Unit: stale peer contributes zero after `stale_timeout`
- Unit: decay curve is monotonic, full-weight window honored
- Unit: future-dated timestamps don't produce negative ages or amplified rates
- Integration: kill one node in a 3-node cluster, verify remaining nodes'
  external rate drops to reflect only live peers within `stale_timeout`

#### 3.2 Lock Contention in Sync Loop

`gossip_sync_loop` takes a write lock on the entire `RateLimitManager` twice per
500ms tick, serializing against the hot admission path.

- [x] **Reduce write-lock scope**
  - Merged the external-rate update and peer-count update into a single pass
    under one write lock (was two passes); both critical sections are now a
    single limiter traversal with no I/O or awaits inside, and gossip I/O
    happens between them
- [x] **Benchmark before/after** (`benches/gossip_contention_bench.rs`,
  `cargo bench --features server --bench gossip_contention_bench`)
  - Decision latency through the shared `RwLock` path, 100k iterations per
    case, gossip sync loop idle vs. active at production 500ms interval:

    | Scopes | Idle p50/p99 | Active p50/p99 | Active max |
    |--------|--------------|----------------|------------|
    | 1      | 167ns / 209ns | 125ns / 167ns | 47µs |
    | 100    | 125ns / 167ns | 84ns / 166ns  | 36µs |
    | 1000   | 83ns / 166ns  | 83ns / 125ns  | 126µs |

  - **Finding: contention is negligible at realistic scope counts.** p50/p99
    are statistically indistinguishable idle vs. active; the only effect is a
    rare max-latency outlier (tens of µs) when a decision lands behind the
    sync loop's write pass. Per-limiter locking / `DashMap` is not justified —
    recorded and stopped, per the task's guidance.

**Tests**: existing tests pass; add a benchmark comparing decision latency with
gossip loop idle vs. active at 1, 100, and 1000 scopes

**Deliverable**: Partition-safe rate aggregation, documented lock behavior

**Verification**:
```bash
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check

# Manual: 3-node cluster, kill node 1, watch node 2/3 external rates decay
```

**Commit Message**: `Milestone 3: Stale peer decay and gossip sync lock fixes`

---

## Milestone 4: Simulation & Testing Architecture

- [x] **MILESTONE COMPLETE** (commit f80370d)

**Goal**: A deterministic multi-node simulator that becomes the project's primary
tool for correctness testing, benchmarking, experimentation, and control-loop
tuning. This is the prerequisite for evaluating alternative engines (Milestone 5)
— without it, comparisons are anecdotes.

**Why**: The existing `request_simulator_plot` example exercises one limiter in
isolation. The interesting dynamics (gossip lag → phase lag → oscillation,
partition overshoot, convergence after churn) only appear with N nodes and a
network model. Real multi-process tests are too slow and nondeterministic to
sweep parameters or run in CI.

### Tasks

#### 4.1 Deterministic Simulation Core

- [x] **Virtual clock**
  - Drive limiters via the existing `update_state_at(Instant)` /
    explicit-timestamp APIs; audit library for any remaining internal
    `Instant::now()` calls on paths the simulator exercises and lift them
    to parameters
  - Simulation advances in fixed ticks (e.g., 10ms); no wall-clock sleeps
- [x] **Simulated cluster**
  - N in-process nodes, each with its own `RateLimiter` + controller
  - Message-bus gossip model with configurable: propagation delay,
    jitter (seeded RNG), message loss rate, partitions (arbitrary node groupings)
  - Reuses the real aggregation/decay logic from Milestone 3 — the simulator
    must exercise production code, not a reimplementation
- [x] **Workload generation**
  - Seeded, reproducible arrival processes: constant, ramp, step, burst,
    Poisson, per-node skew (hot node), per-scope skew (hot key)
- [x] **Determinism guarantee**
  - Same seed + same scenario = identical results, byte for byte
  - This is a hard requirement; it's what makes CI assertions and A/B engine
    comparisons trustworthy

#### 4.2 Scenario Library

- [x] **Scenario definition format** (Rust builder or TOML — pick simplest)
  - Cluster size, workload, target rates, engine + parameters, events timeline
- [x] **Core scenarios**:
  - Steady state: constant load above/below/at target
  - Step change: load doubles at t=30s
  - Ramp: 0 → 3× target over 60s
  - Burst: periodic 10× spikes
  - Node join / node leave mid-run
  - Partition and heal (the Milestone 3 fix must show bounded overshoot
    during partition and re-convergence after heal)
  - Skewed load: one node receives 90% of traffic
  - Scale sweep: 2, 5, 10, 50 simulated nodes

#### 4.3 Metrics & Analysis

- [x] **Per-run metrics**
  - Overshoot: max and time-integrated excess over target
  - Convergence time after each event (within ±5% band of target)
  - Oscillation: variance / peak-to-peak of cluster accepted rate at steady state
  - Fairness: dispersion of per-node accepted rates under uniform load
  - Undershoot: throughput sacrificed below target
- [x] **Output formats — artifacts, not a live GUI**
  - CSV/JSON time series per run for offline analysis
  - Static chart rendering to SVG/PNG (e.g., the `plotters` crate — no GUI
    stack), so results are reproducible and shareable in PRs/docs instead of
    eyeballed on a realtime dashboard
- [x] **Benchmark harness**
  - Run a scenario matrix across engine configs, emit comparison table
  - This is the tool Milestone 5 uses to judge PID vs. Bayesian

#### 4.4 CI Integration

- [x] **Correctness assertions as tests**
  - Encode acceptance thresholds per scenario (e.g., "steady state: cluster
    rate within ±5% of target; step change: converge within 10s; partition:
    overshoot bounded by `stale_timeout × excess demand`")
  - Fast subset in `cargo test` (<30s); full matrix behind `--ignored` or a
    feature flag
- [x] **Placement**: `tests/simulation/` + shared code in a `sim` module or
  dev-dependency-only crate — follow standard Cargo conventions

#### 4.5 Model Checking & Property-Based Verification

Formal methods complement the simulator; they don't replace it. The division of
labor: **quantitative dynamics** (convergence time, overshoot, oscillation) are
continuous-domain properties — simulator territory, model checkers can't
express them. **Discrete protocol logic** (aggregation bookkeeping, membership
accounting, staleness transitions) has safety invariants a model checker can
exhaustively verify over all interleavings, which no finite set of simulation
seeds can.

- [x] **Property-based tests** (`proptest`, already a dev-dependency)
  - Library invariants over arbitrary inputs: token bucket never exceeds
    capacity, refill rate always within [min, max], decay weight monotonic in
    age and within [0, 1], PID output always within output_limit
- [x] **Model-check the aggregation/membership state machine**
  ([`stateright`](https://github.com/stateright/stateright) — a Rust model
  checker, so it exercises the *real* aggregation code rather than a parallel
  spec that drifts)
  - Model: nodes publish/receive/lose gossip messages, crash, partition, heal
  - Safety invariants to verify over all interleavings:
    - A peer's contribution reaches zero within `stale_timeout` of its last
      message (no phantom load, ever — not just in tested scenarios)
    - A node never counts itself, and never counts any peer twice
    - `num_peers` eventually equals the live peer set after quiescence
    - External rate is always ≥ 0 and never exceeds the sum of live peers'
      published rates (given decay weights ≤ 1)
- [x] **TLA+ (deferred, narrow scope)**: not used for M4 — the properties above
  are checkable against real code with stateright, and a separate spec would
  drift. Reserved for the Milestone 9 auth handshake, where
  exhaustive adversarial interleaving analysis is the whole point.

#### 4.6 Retire the GUI Examples

The egui/eframe realtime dashboard (`request_simulator_plot`) was the manual
tuning tool; scripted scenarios with rendered artifacts replace it with
something reproducible, CI-checkable, and shareable.

- [x] Port any load patterns unique to the example into simulator scenarios
  before deleting anything
- [x] Remove `request_simulator_plot` and the dashboard; drop the
  egui/eframe/egui_plot dev-dependencies
- [x] Remove the quick-xml ignores from `.cargo/audit.toml` — they exist only
  for the eframe dependency tree (this also clears most of the audit
  "unmaintained" warnings)
- [x] Update README: Request Simulator section → simulator usage and sample
  output artifacts

**Deliverable**: `cargo test` covers multi-node dynamics deterministically;
a benchmark harness produces engine comparison tables from scenario runs

**Implementation notes**:
- `src/sim/` behind a new no-dependency `sim` feature; `gossip::aggregate`
  now compiles under `server` *and* `sim` so the simulator runs the
  production decay code. RNG is an in-repo SplitMix64 (Vigna's reference
  implementation) so the stream can never shift under a dependency bump.
- Kept non-GUI examples: `request_simulator` (single-limiter terminal demo)
  and `cluster_load_generator` (drives real clusters). Removed
  `request_simulator_plot` and `cluster_visualizer` plus the whole
  egui/eframe tree; charts are now static SVGs from `cluster_sim --plot`
  (plotters, SVG backend only).
- The stateright model bounds state (2 peers, 7-tick horizon, ≤1 in-flight
  message per peer) for exhaustive search (~4.5k states); "no partition"
  in the model because a partition is indistinguishable from message loss
  at the observer, which is modeled.
- **Finding (fixed)**: the scenario matrix exposed unbounded PID integral
  windup in production limiter defaults — a partitioned minority whose fair
  share exceeds its offered load winds up its integral and overshoots for
  ~60s after heal. `ScopePattern` now applies `error_limit = 0.2 × target`
  (sweep of {0.1, 0.2, 0.5}: all bound windup, marginal trade-offs; 0.2 is
  the midpoint). Post-heal re-convergence: never→6s in the partition
  scenario; ramp integrated overshoot 8238→860 requests.
- Milestone-4 matrix at seed 42 (defaults), for future comparison: partition
  overshoot 9506 req (bound: 12000), leave re-convergence 12.0s
  (stale_timeout + PID settle), scale_50 convergence 42.5s, skew never
  converges (equal division cannot serve a 90% hot node — known limitation,
  addressed by demand-weighted division work in Milestone 5+).

**Verification**:
```bash
cargo test --all-features              # includes fast simulation suite
cargo test --all-features -- --ignored # full scenario matrix
cargo run --features sim --example cluster_sim -- --scenario partition --seed 42 --plot
# Re-run with same seed: identical output
```

**Commit Message**: `Milestone 4: Deterministic multi-node simulator and scenario suite`

---

## Milestone 5: Pluggable Control Engine & Bayesian Estimation

- [x] **MILESTONE COMPLETE** (commits 256eda1 → 11d0d6d → be94883 →
  06984f7 → 0af0622 → c8460ea)

**Goal**: Make the control engine swappable behind a trait and build three
candidate engines: PID, pure Bayesian (estimate-and-set), and the Kalman→PID
hybrid. No candidate can be declared best on paper — the separation principle
that would make the hybrid provably optimal assumes a linear plant, Gaussian
noise, and no delay, all of which gossip coordination violates (and LQG-style
designs have no guaranteed stability margins even when the assumptions hold).
So all three go through the Milestone 4 scenario matrix.

The engine is always an **explicit config option** — never selected at runtime.
Benchmarks decide only which value ships as the documented recommended default.

**Baseline to beat**: the Milestone 4 matrix at seed 42 with production PID
defaults (recorded in the Milestone 4 implementation notes) — notably
partition overshoot 9506 requests, leave re-convergence 12.0s, scale_50
convergence 42.5s, and the skew scenario never converging (see 5.3).

**Architecture Reference**: update [docs/architecture.md](architecture.md) with an
Engine section as part of this milestone.

### Tasks

#### 5.1 Engine Abstraction

- [x] **Define `RateController` trait** (library, no server deps)
  - Inputs per update: local accepted rate, per-peer observations
    `(rate, age)`, live peer count, cluster target, elapsed time
  - Output: new local refill rate (clamped to min/max by the caller)
  - The trait boundary is deliberately narrow: engines see observations,
    not gossip internals
- [x] **Port PID behind the trait**
  - Existing `PIDController` becomes `PidEngine`; zero behavior change,
    verified by existing tests and simulator baselines
- [x] **Config selection**
  - `engine = "pid" | "bayesian" | "hybrid"` per scope pattern in TOML, with
    engine-specific parameter tables; explicit config only, no runtime
    auto-selection

#### 5.2 Bayesian Rate Estimator Engine

Frame the problem as state estimation: each peer's true rate is a latent
variable observed through delayed, noisy gossip samples.

- [x] **Per-peer estimator**
  - Model peer rate as a random walk; scalar Kalman filter per (peer, scope)
  - Observation update on each gossip sample; process noise grows the
    variance between samples, so **staleness = uncertainty** (this subsumes
    Milestone 3's decay heuristic with a principled equivalent)
- [x] **Global estimate**
  - Cluster rate = sum of peer means + local rate; total variance = sum of
    peer variances
- [x] **Uncertainty-aware admission**
  - Compute local refill rate from the estimate's upper confidence bound
    (configurable z, e.g., admit against `mean + 1σ`): the node is
    automatically conservative exactly when information is stale (partition,
    churn) and aggressive when the estimate is tight
- [x] **Document the math**
  - Derivation, assumptions (Gaussian noise, random-walk dynamics), parameter
    meanings (process noise ↔ how fast peer rates are believed to change)
  - Cite sources per constants-verification policy (standard Kalman filter
    references, not blog posts)
- [x] **Optional hybrid**: Kalman-filtered global estimate feeding the PID
  engine — cheap to build once both halves exist; include in the benchmark
  matrix

#### 5.3 Engine Benchmark & Selection

- [x] **Run the full Milestone 4 scenario matrix** for each engine
  (`cluster_sim --matrix` grows an engine dimension):
  - Convergence time, overshoot, oscillation, fairness, partition behavior,
    noise robustness (add high-jitter and message-loss scenario variants —
    the gossip model already supports both, the library just doesn't sweep
    them yet)
  - Parameter sensitivity: how badly does each engine degrade when mistuned?
- [x] **Skewed demand as a scoring dimension** — Milestone 4 finding: under
  the skew scenario (one node receives 90% of 2× target load), equal
  division serves only ~60% of the cluster target (the hot node clamps at
  `max_rate / num_nodes`, fairness CV 0.65, never converges). Engines
  receive per-peer `(rate, age)` observations, so demand-weighted share
  division is inside the trait boundary — score engines on throughput
  achieved under skew, not just fairness under uniform load
- [x] **Write up results** in `docs/engine-comparison.md` with plots
  (SVG artifacts from `cluster_sim --plot`)
- [x] **Pick the default engine** based on data; keep the other available
  via config
- [x] **Hot-path check**: engine update runs in the sync loop (per second per
  scope), not per request — verify no regression to the ~40ns decision path

#### 5.4 Simulator-Driven Control Fixes

Defects and scaling laws surfaced by Milestone 4's capacity sweeps
(measurements and re-run instructions in
[docs/capacity-model.md](capacity-model.md)); each follows the anti-windup
precedent: sweep in the simulator, ship the derived default.

- [x] **Rate-estimator floor at sparse per-node shares**: below ~5 rps/node
  fair share, the 1s sliding-window estimator mostly sees an empty window,
  under-measures, and the PID over-admits ~0.6 rps/node (measured +98%
  steady overshoot at 0.75 rps/node share; <2% at 100 rps/node). Candidate
  fixes: adaptive/longer measurement window at low rates, EWMA or
  inter-arrival estimator. Critical for Milestone 6 (per-user shares are
  tiny by design). Characterized by
  `capacity_per_node_share_floor_characterization` — flip that test when
  fixed
- [x] **Gain scheduling vs. fleet size**: convergence ≈ 0.7s × nodes at
  fixed gains (equal division hands each node error/n). Candidate: scale
  integral gain with `1 + num_peers` to restore n-independent settling;
  sweep for stability margins before shipping
- [x] **Cold-start fair-share initialization**: a joining node starts with
  `bucket_capacity = cluster_target` tokens and `refill = cluster_target`,
  admitting a ~full-bucket burst per join (~250 excess requests/join in the
  autoscale scenario; 27 rapid joins cost 8093 requests of overshoot vs a
  ~1300 baseline). Candidate: initialize bucket and refill at
  `target / (num_peers + 1)`; sweep for slow-start cost on legitimate joins

**Deliverable**: Two production engines behind one trait, a data-backed default,
and a written comparison

**Implementation notes** (full data in
[docs/engine-comparison.md](engine-comparison.md) and
[docs/capacity-model.md](capacity-model.md)):
- `src/engine/`: `RateController` trait + `PidEngine` / `BayesianEngine` /
  `HybridEngine`; `staleness_weight` moved into the engine module (gossip
  re-exports it). PID port verified byte-identical against the Milestone 4
  matrix at seed 42. One deviation from the task text: engines own the
  min/max clamping (distributed bounds scale with the live node count,
  which only the engine knows); the caller sanitizes to non-negative
  finite.
- Engine selection: `NENYA_DEFAULT_ENGINE` env var + `ScopePattern.engine`
  and estimator-parameter fields (the future TOML tables will expose the
  same fields); TOML config itself remains a Milestone 7-9 item.
- **Default: `pid`** — best all-round convergence at shipped tuning and
  the only engine whose feedback consumes no gossip data. Hybrid: least
  overshoot, gracefully tolerates 4× mistuned gains (pid limit-cycles).
  Bayesian (`q=1, r=100, z=1`, sweep-derived): serves 87% of achievable
  throughput under 90%-hot-node skew vs pid's 54% — the demand-weighted
  niche — at the cost of UCB undershoot and no band convergence at 10+
  nodes under uniform load.
- **5.4a estimator floor — fixed**: adaptive-window floor
  (`min_window_samples = 20`, swept over {0,2,5,10,20,30,50}) cuts
  sparse-share over-admission from +16–18% to +2.5–3.3%;
  `capacity_per_node_share_floor_fixed` guards it (with a K=0 control run).
- **5.4b gain scheduling — rejected by the sweep**: `ki × n` gives +53%
  chronic over-admission at 100 nodes (+22% even with the anti-windup
  clamp scaled to constant authority). No scheduling knob ships; negative
  result recorded in capacity-model.md.
- **5.4c cold start — fixed, and it flattened the scale law**: bucket
  capacity now tracks `refill × 1s` (library default; explicit
  `bucket_capacity` pins static). Autoscale join overshoot 8093 → 2087
  requests, and the Milestone 4 "0.7s × node count" convergence law turned
  out to be the banked cold-start burst: convergence is now ~4s flat at
  10/50/100 nodes.
- Hot path: warm decision 31ns unchanged; cold-start +8ns (one-time Box
  allocation per scope).

**Verification** (all run at completion):
```bash
cargo test --all-features                                   # 18 suites green
cargo test --all-features --release -- --ignored            # full matrix + sweeps
cargo run --features sim --example cluster_sim -- --matrix --seed 42
cargo fmt -- --check && cargo clippy --all-targets --all-features -- -D warnings
```

**Commit Message**: `Milestone 5: Pluggable control engines with PID vs Bayesian benchmark`

---

## Milestone 6: Per-User Scale — Two-Tier Coordination

- [x] **MILESTONE COMPLETE**

**Goal**: Support millions of per-user scopes per cluster. Per-user distributed
throttling is the core value proposition — per-client/service limits can often
be engineered around by the calling team; per-user limits can't. Naive gossip
replicates every scope to every node and tops out around thousands of scopes.

**Why this can work**: two observations.
1. Load balancers spread a user's traffic roughly uniformly, so
   `local_rate × num_nodes` is a good local estimate of that user's cluster
   rate — each node can cheaply detect which users are anywhere near their limit
2. API usage is heavy-tailed (Pareto-like): at any instant only a small
   fraction of users are near their limit. The tail needs no coordination —
   local enforcement of the equal share `limit / num_nodes` is already accurate
   for it

**Architecture Reference**: [docs/architecture.md](architecture.md) -
Two-Tier Coordination section

### Tasks

#### 6.1 Two-Tier Enforcement

- [x] **Tail tier (default)**: local-only enforcement of the user's equal share
  (`limit / num_peers`); no gossip state; compact limiter representation
  (token bucket only — full engine state allocated on promotion)
- [x] **Hot tier**: scopes promoted into full gossip coordination exactly as
  today
- [x] **Promotion**: when estimated cluster utilization crosses a threshold —
  `local_rate ≥ promote_utilization × limit / num_peers`. `promote_utilization`
  is per-pattern config; the shipped default is **derived, not invented**: the
  safe value is roughly `1 − (max ramp during promotion lag + routing-estimate
  error margin)`, so run a benchmark-harness sweep across Pareto workloads and
  pick the knee of the promoted-set-size vs. worst-case-overage curve. Publish
  the curve in the docs so users tuning the knob can see the tradeoff
- [x] **Demotion with hysteresis**: sustained estimated utilization below
  `demote_utilization` (per-pattern config, default derived from the same
  sweep — wide enough below promotion to prevent flapping) for M seconds
- [x] **Transport-agnostic sync logic**: keep promotion/aggregation/decay
  separable from Chitchat specifics — the same loop must later run against a
  blackboard store (see Future Work: alternative coordination transports)
- [x] **Per-node gossip budget**: hard cap K on gossiped scopes; evict
  lowest-utilization on overflow and log it (no silent truncation)
- [x] **Per-scope gossip keys + compact encoding**: today the whole
  `GossipState` is one JSON blob under a single chitchat key, so any change
  retransmits everything — defeating Scuttlebutt's per-key delta sync.
  Measured baseline (Milestone 4): ~115 bytes/scope, dominated by the
  serde-JSON `SystemTime` (~60 bytes) that is only ever used as an opaque
  change marker. Move to one chitchat key per gossiped scope (anti-entropy
  then ships only changed scopes) and a compact value encoding; a version
  counter can replace the timestamp outright. Note: gossip cost scales with
  scopes × peers and is independent of tps — this item and the budget above
  are what make the hot tier cheap. Before quoting any scope ceiling, verify
  on a **real** 2-node cluster at ~10k scopes that the current monolithic
  blob propagates at all: chitchat gossips over UDP with MTU-bounded
  messages, and the simulator's abstract transport cannot answer this
  (see docs/capacity-model.md)

#### 6.2 Tail Visibility

- [x] **Per-pattern tail aggregate**: each node gossips one number per pattern
  (sum of unpromoted scope rates) so service-level/global limits still see
  total cluster volume
- [x] **Evaluate (don't assume) a tail sketch**: a count-min sketch of tail
  rates gossips at fixed size regardless of user count, merges by addition,
  and answers "approximate cluster rate for ANY user" with one-sided error
  (overestimates → conservative throttling). Decide from simulator data
  whether promotion + aggregate suffices or the sketch earns its complexity

#### 6.3 Memory at Cardinality

- [x] Compact tail-scope state; measure bytes/scope (target: ~10⁶ tail scopes
  in the low hundreds of MB)
- [x] TTL eviction of idle scopes (pulled forward from production hardening)
- [x] Stress benchmark: 1M+ scopes with churn

#### 6.4 Simulator Validation

- [x] New scenarios: Pareto-distributed user traffic (10⁵–10⁶ users), a user
  ramping tail → hot → tail, sticky-routing skew (session affinity breaks the
  uniform-routing estimate — quantify worst-case overage)
- [x] Assertions: no unpromoted user exceeds their limit beyond the documented
  bound; promoted set size ≪ user count; gossip payload bounded by K
- [x] Model/property checks: promotion/demotion state machine — no flapping
  under hysteresis, no scope counted in both the tail aggregate and the hot
  tier (double count)

**Error bound to document honestly**: an unpromoted user can only exceed their
limit via routing skew or during promotion lag (one sync interval + gossip
propagation). Heavy routing skew itself triggers promotion — the hot node sees
the elevated local rate — so the exposure is transient. Quantify both in the
simulator and publish the numbers.

**Implementation notes** (measurements and derivations in
[docs/capacity-model.md](capacity-model.md); architecture in
[docs/architecture.md](architecture.md)):
- `src/gossip/tier.rs`: transport-agnostic policy (compiled under `server`
  and `sim`) — `TailScope` (48 B: token bucket + two-bucket rate
  estimator), promotion test, `DemotionTracker` hysteresis, budget
  eviction. Server (`RateLimitManager` tiered `ScopeEntry`) and simulator
  run the same code; the stateright model checks the same functions.
- **Derived defaults** (`tier_threshold_sweep`, seed 42):
  `promote_utilization = 0.5`, `demote_utilization = 0.25` (highest
  flap-free value), `demote_hold = 10 s`, `estimator_window = 8 s` (first
  window where sparse-rate Poisson clumping stops promoting sub-threshold
  users — a 1 s window promoted 78 scopes per 100k where ~10 were real).
  Key negative result: the anticipated promoted-set-size vs. worst-case-
  overage knee does not exist — unpromoted overage is structurally zero at
  every threshold (per-node cap `limit/n`; skew promotes earlier, not
  later), so the threshold trades hot-set size against coordination
  headroom only.
- **Wire**: per-scope chitchat keys (`s:<scope>`, ~21 B payload vs. ~115 B
  in the old monolithic JSON blob), `t:<pattern>` tail aggregates,
  `nenya_v` counter replacing the wall-clock change marker; change-
  suppressed publishes so anti-entropy ships deltas; `GossipState`
  deleted. Real 2-node UDP at 10k scopes: incremental propagation keeps
  pace; found and documented a chitchat/macOS datagram-size stall
  (hardcoded 65 507 B deltas vs. `net.inet.udp.maxdgram=9216`).
- **Peer-triggered promotion** is gated on the demotion threshold —
  without the gate, staggered demotion flaps (a dying scope's lingering
  peer key re-promotes it around the ring).
- **Memory**: 1M tail scopes = 356 B/scope RSS, 244 ns/create; TTL
  eviction (default 60 s, lossless past the estimator window) bounds the
  resident set by the active window (2M-user churn test).
- **Promotion continuity**: `RateLimiterBuilder::initial_refill_rate` —
  promoted limiters start at the enforced share with tail tokens carried
  over; worst ramp transient measured 1.6–2.2 × limit for one second.
- Count-min sketch: **rejected** — zero unpromoted overage under uniform
  and sticky routing leaves it nothing to protect; sticky mid-band
  under-service (0.40 worst served/offered) is equal-division behavior,
  addressable by demand-weighted engines, not a sketch.

**Deliverable**: millions of per-user scopes with bounded gossip payload,
bounded memory, and a documented worst-case overage bound

**Verification** (all run at completion):
```bash
cargo test --all-features                                    # 20 suites green
cargo test --all-features --release -- --ignored             # matrix, sweeps, stress
cargo test --all-features --release --test scale_stress -- --ignored --nocapture
cargo test --all-features --test gossip_wire -- --ignored    # real 2-node UDP
cargo run --features sim --example cluster_sim -- --scenario pareto_users --seed 42
cargo fmt -- --check && cargo clippy --all-targets --all-features -- -D warnings
```

**Commit Message**: `Milestone 6: Two-tier coordination for per-user scale`

---

## Milestone 7: Client SDKs & API Stabilization

- [ ] **MILESTONE COMPLETE**

**Goal**: Make the "guard clause" trivial in the most popular service languages.
A transparent proxy is explicitly out of scope for now (see Future Work); thin
SDKs deliver most of the ergonomics for a fraction of the effort.

### Tasks

#### 7.1 API Contract

- [ ] **Stabilize the HTTP API**
  - Review request/response shapes for forward compatibility
    (additive-only evolution; version header or `/v1/` prefix)
  - Write an OpenAPI spec in `docs/api/openapi.yaml`; add a CI check that
    the spec matches the handlers
- [ ] **Define client failure policy semantics**
  - Sidecar unreachable or slow: SDKs default to **fail-open** (admit) with
    a configurable timeout (default ~5ms) — a rate limiter must not become
    an availability dependency
  - Document this tradeoff explicitly

#### 7.2 SDKs

Each SDK is deliberately tiny (~100 lines): one call, one middleware/decorator,
timeout + fail-open, zero heavy dependencies.

The target endpoint is configurable: localhost sidecar by default, or a remote
nenya service URL ("service mode") — this is how serverless callers (AWS
Lambda etc.) connect; see Milestone 8.

- [ ] **Rust**: `should_throttle(scope)` client in the nenya crate behind a
  `client` feature (no server deps); Tower middleware example
- [ ] **Python**: `pip` package — plain function + decorator + ASGI middleware
  example (FastAPI/Starlette)
- [ ] **Node.js/TypeScript**: `npm` package — plain function + Express/Fastify
  middleware
- [ ] **Go**: module — plain function + `net/http` middleware
- [ ] **Java/Kotlin** (stretch): plain client + servlet filter example

Repository layout: `sdks/<language>/` in this repo (monorepo keeps API and SDKs
in lockstep while everything is 0.x).

- [ ] **SDK conformance tests**
  - One shared test spec (scenarios as JSON) each SDK runs against a real
    sidecar binary in CI
- [ ] **Scope ergonomics**: the scope/partition argument is optional — omitted,
  the decorator uses a single service-wide scope; provided (static string or
  per-request lambda), it keys per-client/per-user scopes. Per-client
  isolation with a default limit and per-key overrides is the existing scope
  pattern system (`client#*` default + `client#big_corp` override) — document
  this pairing prominently
- [ ] **Docs**: per-SDK README with the guard-clause example front and center:
  ```python
  @nenya.throttle(scope=lambda req: f"client#{req.client_id}")
  def handler(req): ...
  ```

#### 7.3 Cost-Weighted Limiting (LLM-Aware)

Request-count rates are the wrong dimension for LLM-backed APIs: cost varies
~100× between requests and the scarce resource is an upstream quota (model
TPM, AgentCore TPS) or dollars. Make cost a first-class rate dimension:

- [ ] **Library**: sliding window accumulates weights, not counts
  (weight 1.0 = today's behavior; additive change)
- [ ] **Post-hoc usage recording**: actual cost (e.g., LLM tokens) is known
  only after the response — add `POST /record_usage {scope, cost}`; the
  decision endpoint stays predictive, admitting against the rate of recorded
  cost
- [ ] **SDK helpers**: decorator variant that records usage on completion
- [ ] **Config**: per-pattern `rate_dimension = "requests" | "cost"` with the
  target in cost units/sec
- [ ] **Document the flagship pattern — upstream quota arbitration**:
  `cluster_target` = an externally imposed provider quota (Bedrock model TPM,
  AgentCore `InvokeAgentRuntime` TPS, Gateway invocations/sec); scopes =
  users. The fleet collectively converges under the quota (no provider 429s,
  smoothed demand curve) while no single user starves the rest. The
  soft-limit caveat doesn't apply here: the upstream enforces the hard cap;
  nenya's job is fair division and smoothing

**Deliverable**: Published (or publish-ready) SDKs for Rust, Python, Node, Go
with conformance tests in CI, and cost-weighted limiting end to end

**Verification**:
```bash
cargo test --all-features
./sdks/run-conformance-tests.sh    # spins up sidecar, runs all SDK suites
```

**Commit Message**: `Milestone 7: Client SDKs (Rust, Python, Node, Go) with conformance suite`

---

## Milestone 8: Platform Deployment & Discovery

- [ ] **MILESTONE COMPLETE**

**Goal**: The "one line per platform" experience — a container image plus a
copy-paste snippet for Docker Compose, Docker Swarm, Kubernetes, and AWS ECS,
with peer discovery handled automatically — plus the flagship AgentCore quota
arbitration integration (8.3).

**Architecture Reference**: [docs/architecture.md](architecture.md) - Discovery
Layer, Deployment Patterns

### Tasks

#### 8.1 Discovery Trait & Implementations

**Design principle**: gossip needs only *one* live seed — anti-entropy
membership propagation enumerates the rest of the cluster. Discovery finds
candidates, not the full topology. This makes generic DNS the universal
mechanism and everything else an ergonomic layer on top.

- [ ] **`PeerDiscovery` trait** with provider chaining: configured providers
  run in order, results are unioned and deduped; if nothing yields a seed,
  start standalone with a prominent warning (never crash)
- [ ] **Tier 1 — DNS seed resolution** (the universal path): resolve one
  configured name to A/AAAA records. One code path covers Kubernetes headless
  services, Swarm `tasks.<service>`, Compose service names, ECS Cloud Map,
  and Route53/custom DNS over EC2 or bare-metal nodes
- [ ] **Tier 2 — static seed list**: config + `NENYA_SEED_NODES` env; always
  available, always the fallback
- [ ] **Tier 3 — optional platform providers** (for what DNS can't express):
  - Kubernetes API (`kube` endpoints + label selector)
  - EC2 instance tags via the AWS API — the "cloud auto-join" pattern proven
    by Consul/Nomad (`provider=aws tag_key=...`); covers standalone-EC2
    deployments with no DNS setup
  - Docker API (`bollard`) only if Swarm DNS proves insufficient
- [ ] **mDNS — opt-in only, never an automatic fallback**: multicast is
  unavailable in most cloud networks (AWS VPCs don't forward it; most K8s
  CNIs and Docker overlay networks drop it), so as a fallback it would fail
  silently exactly where people deploy. On flat L2 networks (bare metal,
  labs) it's a good zero-config option — behind explicit
  `discovery = "mdns"` config
- [ ] **Symmetric bootstrap — no special first node**: every node ships
  identical config (same DNS name / tag query, which includes itself;
  self-address filtered out). No ordered "start the seed, then the rest"
  setup — that would break the one-line sidecar ergonomics. When dynamic
  discovery is used, every live node is a seed; a fixed seed being offline
  during autoscale is only possible with a static list
- [ ] **Continuous join, not startup-only**: re-resolution never stops and
  join retries with backoff, so nodes that cold-start into a singleton
  cluster (e.g., DNS registration lag) merge automatically once discovery
  surfaces peers. Discovery failure must never affect already-joined members
  (membership is gossiped; discovery only matters for joining)

**Tests**: mocked DNS/API responses per provider; provider chaining and
dedup; standalone fallback on total discovery failure; cold-start race —
N nodes started simultaneously with lagged/partial DNS answers converge to
a single cluster

#### 8.2 Packaging

- [ ] **Official Docker image**: multi-arch (amd64/arm64), distroless or
  scratch base, published via CI on tags
- [ ] **Per-platform snippets** in `deploy/`, each tested end-to-end:
  - `deploy/compose/`: one service block to paste into `docker-compose.yml`
  - `deploy/swarm/`: service definition with Swarm discovery preconfigured
  - `deploy/kubernetes/`: native sidecar (initContainer with
    `restartPolicy: Always`, K8s ≥1.28) + headless service manifest
  - `deploy/ecs/`: task definition with container dependency ordering
  - `deploy/service/`: **service mode** — nenya as a small standalone regional
    cluster (e.g., 3 Fargate/ECS tasks behind an NLB) for callers that can't
    host a sidecar. AWS Lambda and other serverless runtimes call it via the
    SDK's remote endpoint config; include a working Lambda example
- [ ] **Zero-config defaults**: sidecar starts useful with no TOML — sane
  default scope pattern, discovery auto-detected from environment
  (K8s service account present → kubernetes; ECS metadata endpoint →
  ecs; else static)

#### 8.3 AgentCore Quota Arbitration (Flagship Integration)

The highest-value integration and the priority serverless target: fleet-wide
per-user fair division of AgentCore's account/resource-level quotas is a real
gap with no AWS-native solution (verified July 2026 — quotas are per
agent/account, WAF and usage plans are too coarse, Spring AI's limiter is
single-process). Generic Lambda-protection adapters are deliberately deferred
behind this — see Future Work for the ordering.

- [ ] **End-to-end example**: per-user arbitration of `InvokeAgentRuntime`
  TPS and Gateway invocation quotas — `cluster_target` = the account quota,
  scopes = users, cost-weighted (Milestone 7.3) where token counts matter
- [ ] **SDK guards at the AgentCore call sites** (Python first — the dominant
  agent language): wrap `InvokeAgentRuntime` / Gateway MCP tool calls with
  the throttle check + usage recording on completion
- [ ] **Both caller topologies supported**:
  - Containers (ECS/K8s services calling AgentCore): sidecar + gossip
  - Lambda callers, and AgentCore Runtime agents themselves (session-scoped
    microVMs — no gossip inside, same constraints as Lambda): service mode
    via the SDK's remote endpoint
- [ ] **CDK constructs**: `NenyaService` (service-mode cluster: Fargate +
  NLB) and `NenyaThrottle` (wire SDK endpoint/env into an existing Lambda,
  ECS service, or agent) — "protect your AgentCore quota" in a few lines
- [ ] **Runbook**: deriving targets from Service Quotas values, tuning
  per-user shares, and monitoring upstream 429s as the ground-truth signal
  that arbitration is working

#### 8.4 End-to-End Platform Tests

- [ ] **Compose**: `docker compose up` a 3-node cluster + demo app in CI
- [ ] **Kubernetes**: kind-based CI job — deploy, scale 1→3, verify
  discovery and coordination
- [ ] **Swarm / ECS**: scripted manual test procedures documented in
  `deploy/README.md` (CI if practical)

**Deliverable**: `docker pull` + one pasted block = running distributed rate
limiter on each platform

**Verification**:
```bash
cargo test --all-features
docker compose -f deploy/compose/demo.yml up   # 3 nodes coordinate
# kind CI job green
```

**Commit Message**: `Milestone 8: Platform deployment packaging and peer discovery`

---

## Milestone 9: Security & Authentication

- [ ] **MILESTONE COMPLETE**

**Goal**: Secure gossip with cluster secret authentication.

**Architecture Reference**: [docs/architecture.md](architecture.md) - Security Model

### Tasks

#### 9.1 Cluster Secret Authentication

- [ ] **Secret loading**: file (`/run/secrets/nenya_cluster_secret`), env
  (`NENYA_CLUSTER_SECRET`), TOML (`cluster_secret_file`); error if absent
- [ ] **Message authentication**: HMAC over gossip payloads with the cluster
  secret; reject unauthenticated/tampered messages
  (may require wrapping Chitchat's transport layer)
- [ ] **Join handshake**: challenge-response so nodes with the wrong secret
  never enter the peer set
- [ ] **TLA+/model-checked handshake** (recommended): specify the
  challenge-response protocol and check safety (no unauthenticated node ever
  enters the peer set) against replay, reordering, and message-drop
  interleavings before implementing

**Tests**: correct secret joins; wrong secret rejected; tampered payload rejected

#### 9.2 TLS for Gossip (Optional)

- [ ] mTLS transport wrapper with configurable cert/key/CA paths

**Deliverable**: Only authenticated nodes participate in coordination

**Commit Message**: `Milestone 9: Cluster secret authentication`

---

## Milestone 10: Production Hardening & v1.0.0

- [ ] **MILESTONE COMPLETE**

**Goal**: Production-ready release.

**Architecture Reference**: [docs/architecture.md](architecture.md) - Failure
Modes, Performance Characteristics

### Tasks

#### 10.1 Resilience

- [ ] Graceful degradation: gossip failure → local-only limiting, no crash;
  discovery failure → static seeds with exponential backoff
- [ ] Graceful shutdown: drain in-flight requests, notify peers, clean SIGTERM
- [ ] Health semantics: `/health` unhealthy on stale gossip (>10s) or missing
  expected peers
- [ ] Scope TTL cleanup (configurable) to bound memory with high-cardinality
  scopes

#### 10.2 Performance Validation

- [ ] Load tests (wrk2): 1K RPS × 5min p99 <5ms; max throughput >50K RPS/node;
  1000-scope multi-scope run; 3-node distributed load; burst and ramp profiles
- [ ] Stress: 10K scopes under load, stable memory, no leaks
- [ ] Soak: 24h at 5K RPS, no crashes/leaks/latency drift
- [ ] Benchmarks vs. Milestone 0 baseline: no regressions
  (~40ns decision, <500μs handler p99, <100μs gossip overhead)
- [ ] Simulator high-rate regime run (`cargo test --all-features --release --
  --ignored test_high_rate_regime_1m_tps`) as part of perf-validation rituals.
  Milestone 4 measurements to compare against: dynamics rate-invariant from
  300 rps through 10M tps (+0.8% steady bias, 6.5s convergence); simulator
  sustains ~100M simulated requests/s; memory is the first wall (~86MB RSS
  at 1M tps, ~9.7GB at 100M tps) because the sliding window stores one
  16-byte `Instant` per accepted request per `update_interval` and
  `bucket_capacity` defaults to a full cluster-second of tokens. If embedded
  high-rate use (≫100K accepted rps/node) becomes real, replace the
  timestamp window with a fixed-bucket counting estimator — until then the
  O(rate) window is fine at sidecar rates

#### 10.3 Documentation & Release

- [ ] Config reference, runbook (secret rotation, gossip debugging,
  common failures), per-platform deployment guides (from Milestone 8)
- [ ] Soft-limit disclosure: document worst-case overshoot
  (`propagation_delay × excess demand`) and that nenya targets fairness and
  overload protection, **not** billing-grade quota enforcement
- [ ] CI/CD: full pipeline green, Docker images published, binaries released,
  tag v1.0.0

**Performance Targets**:
- Library decision: ~40ns | HTTP handler: <500μs p99 | End-to-end: <5ms p99
- Throughput: >50K RPS/node | Memory: <100MB @ 1K scopes | CPU: <50% @ 10K RPS

**Commit Message**: `Milestone 10: Production hardening and v1.0.0 preparation`

---

## Future Work (Post-v1.0)

Deliberately deferred; prioritize from real usage.

- **Resource-based limiting** (CPU/memory): deferred by design. This is
  admission control, not rate limiting — the rate→resource relationship is
  nonlinear and workload-dependent, and the systems that do it well (Netflix
  concurrency-limits, Envoy adaptive concurrency) control *concurrency* with
  gradient/AIMD, not rate with PID. Likely lands as a sibling engine
  (target CPU% as setpoint, heavy input filtering) plus a concurrency-limit
  mode; needs the simulator extended with a load→resource model first.
- **Transparent proxy mode**: Envoy `ext_authz` adapter over the existing API
  for zero-code-change integration; standalone reverse-proxy mode if demand
  exists
- **Alternative coordination transports (blackboard)**: gossip is one
  transport for the core loop (local enforcement + periodic rate sharing +
  feedback control), not the idea itself. The serverless analog embeds the
  nenya library in the runtime (Lambda layer/extension, Cloudflare Worker)
  and syncs through a shared store instead of a mesh: DynamoDB item per hot
  scope with an instance map + TTLs, ElastiCache, or a Durable Object per hot
  scope. Same engine, same staleness-decay semantics (a frozen environment
  ages out like a partitioned peer); decisions stay local and in-memory — the
  store is touched once per sync interval per instance, never per request.
  Two-tier promotion is load-bearing here: only hot scopes touch the store,
  which bounds DynamoDB cost (tens of dollars/day at millions-of-users scale
  — acceptable; cost is not the blocker). **Why deferred anyway**: for
  LLM-scale workloads (multi-second, dollar-scale requests) service mode's
  1–3ms hop is noise, so the blackboard's one advantage — zero per-request
  network — doesn't bind. Build it when a high-QPS, latency-sensitive
  serverless API demands it. Gossip *inside* function runtimes remains a
  non-goal (frozen environments, no inbound connections, constant churn)
- **Lambda + Bedrock API guarding (first follow-up milestone candidate)**:
  the same arbitration pattern for Lambda services calling Bedrock model APIs
  directly (quota = model TPM/RPM). Mostly composes shipped pieces — service
  mode + SDK guard + `NenyaThrottle` + cost-weighted rates. Promote to a
  milestone once AgentCore integration (8.3) ships
- **Protecting Lambda autoscaling itself (further future)**: the interception
  ladder applied to the *hosting* platform rather than the upstream quota.
  Rungs, cheapest-per-rejected-request first: (1) API Gateway Lambda
  authorizer calling nenya — rejects before the target Lambda is invoked,
  saving the invocation and concurrency slot (caveat: authorizer caching is
  per-identity TTL, so throttle decisions need short/no cache); (2) Lambda
  layer/extension early return — invocation still billed, but a ~5ms 429 vs.
  a multi-second handler cuts occupied concurrency ~100–1000× (concurrency =
  arrival × duration), which is what smooths the autoscaling curve;
  (3) in-handler guard — saves backend calls only
- **Multi-cloud adapters**: Cloudflare Workers (Durable Object transport),
  GCP Cloud Run/Functions and Azure Functions equivalents of the AWS
  adapters; Terraform/Pulumi alongside CDK. Demand-driven — service mode
  already works on all of them day one, since it's just an HTTP call
- **Serverless-optimized clients**: token leasing — the SDK fetches a batched
  allowance per scope, decrements locally, refreshes asynchronously — to
  amortize the network hop to a service-mode cluster; an AWS Lambda Extension
  hosting the lease cache. Service mode (Milestone 8) is the baseline
  serverless path; leases cut its per-invoke overhead to near zero
- **Adaptive engine tuning**: measure gossip lag from timestamps, scale gains
  accordingly (PID), or auto-fit process noise (Bayesian)
- **Shared capacity pools / hierarchical limits**: scopes drawing from a
  common budget (global → service → endpoint)
- **Priority & weighted fairness** between scopes
- **Dynamic reconfiguration**: runtime limit changes via admin API
- **State persistence** for fast restart recovery
- **Multi-cluster namespacing** on shared networks
- **Additional SDKs** (Java/Kotlin GA, Ruby, PHP) by demand

---

## Milestone Summary

| Milestone | Key Deliverable | Status |
|-----------|----------------|--------|
| 0 | Single-crate + HTTP stack + limiter foundation | ✅ Complete |
| 1 | Working HTTP rate limiter | ✅ Complete |
| 2 | Gossip coordination (equal division PID) | ✅ Complete |
| 3 | Gossip correctness fixes (stale decay, locking) | ⏳ Current |
| 4 | Deterministic simulator + scenario/benchmark suite | 🔜 Next |
| 5 | Pluggable engines: PID vs Bayesian, benchmarked | 🔜 Not Started |
| 6 | Two-tier coordination for per-user scale (millions of scopes) | 🔜 Not Started |
| 7 | Client SDKs (Rust, Python, Node, Go) | 🔜 Not Started |
| 8 | Platform deployment + discovery + AgentCore quota arbitration | 🔜 Not Started |
| 9 | Cluster authentication | 🔜 Not Started |
| 10 | Production-ready v1.0.0 | 🔜 Not Started |

**Legend**: ✅ Complete | ⏳ Current | 🔜 Not Started

---

## Success Criteria

### Milestone 3 Complete
- Dead/partitioned peers stop influencing admission within `stale_timeout`
- Gossip loop lock behavior measured and documented
- All tests passing

### Milestone 4 Complete
- Same seed → byte-identical simulation results
- Core scenario suite runs in CI under 30s
- Stateright safety invariants (staleness, no double-count, peer accounting)
  verified and running in CI
- Benchmark harness emits engine comparison tables
- Runs emit CSV/JSON + SVG/PNG chart artifacts; GUI examples and
  egui/eframe dependencies removed, audit ignores dropped

### Milestone 5 Complete
- PID, Bayesian, and hybrid engines behind one trait, selected by explicit
  config (no runtime auto-selection)
- `docs/engine-comparison.md` published with scenario-matrix results
- Documented default engine value chosen from data, not preference

### Milestone 6 Complete
- 1M-scope stress benchmark passes with bounded memory and gossip payload
- Promotion/demotion verified: no flapping under hysteresis, no double-count
  between tail aggregate and hot tier
- Worst-case tail overage quantified in the simulator and documented

### Milestone 7 Complete
- Guard clause is ≤3 lines in Rust, Python, Node, and Go
- All SDKs pass the shared conformance suite against a real sidecar
- Fail-open semantics documented and tested

### Milestone 8 Complete
- One pasted block per platform yields a working cluster
- Discovery verified end-to-end on Compose and kind (K8s)
- Zero-config startup works
- AgentCore arbitration example works end-to-end: per-user fair share under
  the account quota, validated by the absence of upstream 429s under load

### Milestone 9 Complete
- Unauthorized nodes cannot join or influence rates

### Milestone 10 / v1.0.0
- All performance targets met, soak test clean
- Documentation complete, soft-limit semantics disclosed
- CI green, images and binaries published

---

## Risk Mitigation

1. **Simulator fidelity**: simulated gossip may diverge from real Chitchat
   behavior → keep the multi-process integration tests as a reality check;
   validate simulator predictions against a real 3-node cluster once per
   engine change
2. **Bayesian engine complexity**: Kalman + admission control has more knobs
   than PID → the parameter-sensitivity sweep in 5.3 is mandatory, not
   optional; if it's not robustly better, PID stays default
3. **SDK maintenance surface**: four languages is a commitment → keep SDKs
   ≤~100 lines, generated conformance tests, no feature creep
4. **Chitchat transport wrapping** (auth): may be invasive → prototype the
   HMAC wrapper early in Milestone 9; fall back to network-level isolation
   guidance if the library fights it
5. **Platform CI flakiness**: kind/compose jobs can be slow or flaky →
   generous timeouts, retries, keep them out of the default `cargo test` path
6. **Uniform-routing assumption (two-tier)**: sticky/session-affinity load
   balancing skews the `local_rate × N` estimate → skew itself triggers
   promotion on the hot node, and the sticky-routing simulator scenario
   quantifies worst-case overage so the bound is measured, not assumed
