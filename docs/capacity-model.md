# Capacity Model

What bounds a Nenya deployment, with measured coefficients. Everything here
is **simulator- and microbench-derived** (Milestone 4, Apple M-series dev
machine, seed 42) except where marked; real-hardware validation happens in
Milestone 10 and supersedes these numbers. Re-derive with:

```bash
cargo test --all-features --release --test capacity -- --ignored --nocapture
cargo run --features sim --release --example cluster_sim -- --matrix --seed 42
cargo bench
```

## The four independent ceilings

Capacity questions decompose into four ceilings that bind independently.
"Max tps" or "max nodes" alone is underspecified — a deployment hits
whichever ceiling it reaches first.

### 1. Requests/second per node — HTTP stack, not coordination

The rate-limit decision costs ~30ns and gossip traffic is **independent of
request rate** (nodes exchange per-scope rates, not requests), so the
coordination layer imposes no tps ceiling of its own. Per-node throughput is
bounded by the HTTP/JSON stack; the Milestone 10 target is >50K rps/node
(wrk2-validated). Cluster-wide throughput is nodes × per-node — control
dynamics are provably rate-invariant (identical convergence and bias from
300 rps through 10M rps in simulation), so a fleet large enough for millions
of cluster-wide tps is not limited by Nenya's algorithms. Don't put
"millions of tps" in user-facing copy until Milestone 10 measures the
per-node HTTP number on stated hardware; after that, the honest phrasing is
"the coordination layer adds no throughput ceiling: N nodes × measured
per-node rps".

### 2. Node count — no convergence ceiling (law flattened, Milestone 5)

**Convergence is flat in node count: ~4s at 10, 50, and 100 nodes** (2×
target load, production gains). The Milestone 4 law ("≈ 0.7s × node count":
37s @ 50, 72s @ 100, 278s @ 400) turned out not to be a gain problem at
all — it was the cold-start token bucket. Every node initialized a full
cluster-target-sized bucket, so an n-node fleet collectively banked
n × target of burst tokens, drained at target-rate excess ≈ n seconds. The
adaptive burst allowance (`bucket capacity = refill × 1s`, the library
default since Milestone 5.4) removes the banked burst and the law with it.

Two negative sweep results worth keeping (both re-runnable, seed 42):
integral gain scheduling (`ki × n`) — the roadmap's original candidate —
*destabilizes* at scale: +53% chronic over-admission at 100 nodes with the
cluster-scale anti-windup clamp, still +22% with the clamp scaled to
constant integral authority (`error_limit / n`). The equal-division loop's
total response is already n-independent (each node corrects `error/n`; n
nodes sum to one controller's worth), so there was never a gain deficit to
schedule away. Chitchat membership is likewise not a binder at these sizes
(Quickwit operates it at hundreds of nodes).

### 3. Per-node share — rate-estimator floor (fixed, Milestone 5)

**Fixed by the adaptive-window floor** (`min_window_samples = 20`,
`RateLimiterBuilder::min_window_samples`): trimming keeps the last 20
accepted timestamps, so below ~20 rps/node the measurement window stretches
to span real samples instead of reading mostly-empty 1s windows. Swept at
K ∈ {0, 2, 5, 10, 20, 30, 50} across 0.75/3/5/100 rps/node shares (seed 42,
300s runs): steady over-admission falls from **+16–18% (old fixed window)
to +2.5–3.3% at K=20**; K=50 over-remembers at extreme sparsity (+5.4% at
0.75 rps/node), K=10 leaves ~5%. Healthy shares (≥100 rps/node) are
byte-identical. Guarded by `capacity_per_node_share_floor_fixed`, which
also re-runs the K=0 control to keep the original defect reproducible.

Trade-off: the floor adds estimator memory of `K / rate` seconds whenever
the rate is below `K / update_interval`. With production-default gains this
is benign at every regime probed (including 100 ms intervals at 10 rps),
but a deliberately hot tuning (kp = 1.0, 100 ms interval, ~50 rps → 4
update intervals of estimator lag) rectifies into a limit cycle ~25% above
target — aggressive custom tunings at rates below `K / update_interval`
should lower `min_window_samples`. During idle periods the estimate decays
hyperbolically (`K / elapsed`) rather than snapping to zero; peers see the
same decaying value via gossip. Still relevant to Milestone 6 sizing:
promoted per-user scopes now measure accurately down to well below 1
rps/node.

### 4. Scope count — solved by two-tier coordination (Milestone 6)

CPU was never the binder (aggregation ~34 ns per scope-peer per tick; key
encoding <1 µs per 10k changed scopes). The pre-Milestone-6 wire was:
~115 bytes/scope in one monolithic JSON value under a single chitchat key,
retransmitted wholesale on any change. Milestone 6 replaced it and added
the two-tier architecture; the measured state (seed 42, M-series, tests in
`tests/gossip_wire.rs`, `tests/scale_stress.rs`, `tests/simulation.rs`):

- **Wire format**: one chitchat key per hot scope (`s:<scope>` →
  3-decimal rate, ~21 B of key+value for a 13-char scope, ~31 B with
  chitchat's per-KV framing), one key per pattern for the tail aggregate
  (`t:<pattern>`), and a `nenya_v` publish counter as the change marker.
  Values are only re-set when the rounded rate changes, so anti-entropy
  ships deltas; idle hot scopes cost nothing per exchange.
- **Real 2-node UDP verification at 10k scopes**: 10k keys introduced
  incrementally (the realistic promotion pattern) propagate fully with
  the receiver keeping pace (52 s at 200 keys/s introduction; zero lag).
  **Platform finding**: chitchat builds anti-entropy deltas up to a
  hardcoded 65 507 B UDP datagram. On Linux these IP-fragment and a
  backlog drains at ~64 KB per 1 s gossip round per peer; on macOS the
  default `net.inet.udp.maxdgram=9216` makes the send fail with EMSGSIZE
  and — because chitchat always rebuilds the largest possible delta — a
  node whose pending delta exceeds ~9 KB (≈300 keys of burst) stalls
  **forever** while phi-accrual liveness stays green. Dev workaround:
  `sudo sysctl -w net.inet.udp.maxdgram=65535`. Steady-state nenya only
  publishes changed keys, so this bites large bursts (e.g. a node joining
  a cluster with a big hot set), not normal operation on Linux.
- **Tail tier at cardinality**: 1M tail scopes = 356 B/scope RSS
  (`TailScope` itself is 48 B; the rest is the scope-name key, the map,
  and allocator overhead) ≈ 355 MB, built at 244 ns/scope; warm tail
  admits ~256 ns including scope-name hashing. Idle-scope TTL eviction
  (default 60 s, sweeps every TTL/2) bounds the resident set by the
  active-user window: 2M churned users peak at one wave's residency.
- **Promoted set**: Zipf(1.0) over 100k users at 60× a 10 rps per-user
  limit promotes 13 scopes under uniform routing (and ~0 under sticky —
  single-node users generate no peer evidence and need no coordination) —
  the gossip payload is the warm head, not the population.

### Is promotion still needed after delta sync? (ablation)

The per-scope-key wire format removed the original scope-count binder
(retransmitting every scope on any change), so the question deserved a
before/after test rather than an assumption
(`tests/two_tier_ablation.rs`, 300k scopes, 2 peers, release, seed 42):

| | two-tier (default) | all-hot (ablated) |
|---|---|---|
| RSS | 356 B/scope | 949 B/scope |
| steady sync tick (apply+collect, /500 ms) | **1.2 ms** | **204 ms** |
| replicated keyspace / joiner catch-up | ~0 keys | 300k keys ≈ 5.9 MB |
| steady delta wire (600 rps, 100k users) | ~1 KB/s/peer | ~24 KB/s/peer |

**Verdict: still needed, but the binder moved.** Steady-state wire volume
is no longer the problem — delta sync means only value-changing keys
retransmit (~400/s at this traffic, proportional to distinct active
users/sec, not user count). What still rules out all-hot at scale:
1. **Sync-tick CPU**: applying peer observations + collecting rates is
   O(scopes × peers) every 500 ms under the manager write lock — 44% of
   the tick budget at 300k scopes with 2 peers, past the whole tick near
   1M, stalling the decision path.
2. **Replicated keyspace + catch-up**: every node holds every peer's full
   keyset and every joiner must ingest it (~1.5 min at 300k on Linux's
   64 KB/round; a >9 KB burst stalls forever on default macOS).
3. **Traffic-proportional churn**: the delta wire scales with active
   users/sec — fine at 600 rps, MB/s at the 10⁵-rps regimes the capacity
   model targets. Two-tier caps all three at the hot-set size K.

## Two-tier defaults (sweep, seed 42 — evidence-based design)

Derivation data from `tier_threshold_sweep` (re-run:
`cargo test --all-features --release --test simulation tier_threshold_sweep -- --ignored --nocapture`).
Since the evidence redesign, tail scopes are enforced at the **full
limit** locally and promotion requires `local + Σ peer rates ≥
promote_utilization × limit` with nonzero peer evidence; the watch
watermark (`demote_utilization × limit / n`) controls when a tail
scope's rate is published as evidence.

**Promotion-estimator window** (noise → wasted watching/promotion vs.
detection lag): a 1 s window promotes 41 per 100k Zipf users (~10 truly
over threshold); 8 s promotes 13 at 1.0 s ramp-promotion lag; wider
windows shave 1–2 scopes for ~0.5 s more lag. Default **8 s**.

**Promotion threshold** — overage is structurally bounded at every value
(unpromoted cluster rate `< limit × (1 + demote_utilization)`: one full
bucket + n−1 sub-watermark nodes), so the threshold trades hot-set size
against coordination headroom only:

| promote | promoted (100k users) | worst unpromoted rps (limit 10) | sticky worst served/offered | ramp max 1s |
|---------|----------|-------------------------------|------------------------------|-------------|
| 0.3 | 24 | 2.28 | 0.98 | 22 |
| **0.5** | **13** | **3.67** | **0.98** | **30** |
| 0.8 | 7 | 5.58 | 0.98 | 30 |

**Demotion threshold** (also the watch-watermark divisor and the
unpromoted-bound term): a user parked at the demotion boundary for 300 s
→ 0 promotions at 0.25, 6 at 0.35, 15–18 at 0.45. Default **0.25**.

**Sparse-share floor**: splitting a small per-user limit across a large
cluster once starved scopes outright (sub-token adaptive capacities) and
then lost ~40% of a Poisson stream to single-token clumping. The
adaptive bucket capacity floors at 4 tokens (swept 1/2/4/8 → served
0.62/0.84/0.94/0.97 for an 8 rps user on 25 nodes; service-scale
scenarios unchanged). Fixed-point measurements of the evidence design:
8 rps user on 25 nodes served 0.94 of offered; autoscale (service
L=300) join overshoot 3079 vs the 4000 budget; a 20-request single-node
burst admits the full 10-token limit bucket with no promotion.

**Routing strategies — sticky is now the good case**: with full-limit
tail buckets and evidence-gated promotion, session-affinity users are
served at ~0.98 of offered with no promotion at all (they cannot exceed
the limit through one bucket). Uniform/round-robin/least-loaded spread
traffic generates evidence and coordinates normally
(`test_routing_strategies_preserve_two_tier_invariants` asserts the
unpromoted bound under all four policies). The promotion-lag transient
for a spread step is `min(offered, n × limit)` for ~1 second (the
`user_ramp` scenario measures it).

**Compact tail sketches (count-min / Bloom) — evaluated, not shipped**:
under the previous share-based design there was nothing for a sketch to
protect (overage was structurally zero). Under the evidence design the
question is live in one specific regime: clusters with more concurrently
warm *spread* users than the gossip budget K, where dropped watch
entries weaken the evidence channel and the unpromoted bound degrades
toward per-node enforcement for the dropped scopes. Until that regime is
demonstrated, per-scope watch keys + the budget are simpler and
sufficient; revisit with data if it materializes.

## Sizing formula (per node)

```
memory  ≈  tail_scopes × ~360 B                (measured; TailScope 48 B + key + map)
         + hot_scopes × ~1 KB                  (full limiter + engine)
         + accepted_rps × 16 B                 (sliding window, one update_interval deep)
gossip  ≈  changed_hot_scopes × ~31 B × 2/s    (per active peer link; ≤ K × 31 B × 2/s,
                                                K = gossip budget, default 1000 ⇒ ≤ ~62 KB/s)
CPU     ≈  rps × 30 ns  (decisions; tail admit ~256 ns incl. scope-name hash)
         + hot_scopes × 34 × peers ns / 500 ms (sync loop)
         + HTTP stack (dominant; Milestone 10 measures)
```

Cold-start (fixed, Milestone 5.4): bucket capacity now tracks the control
engine's output (`refill × 1s`) instead of staying at a static
cluster-target size, so a joining node's burst allowance shrinks to its
fair share within one control update. Swept variants (zero initial tokens,
capacity tracking, both): tracking is what matters — autoscale join-burst
overshoot 8040 → 2076 requests, burst-scenario overshoot 5972 → 2452, at
the cost of ≤0.1% steady throughput (undershoot 25 → 2147 requests over a
300s 1M-request run at 100 nodes). An explicit
`RateLimiterBuilder::bucket_capacity` pins a static capacity for callers
that want a fixed burst budget; the first second of a true cold start
still admits up to one `target` of burst (capacity starts at the
configured default until the first engine update).

## What the simulator deliberately does not model

The gossip transport is an abstract message bus (delay + jitter + loss +
partitions). Not modeled: UDP/MTU fragmentation, chitchat anti-entropy
rounds, kernel queues, packet reordering effects on versioned KV state,
serialization on the wire. This is the right trade for a *soft*, latest-wins,
2 Hz protocol with a 10s staleness horizon: transport pathologies below
~1s are invisible to every mechanism in the system (verified: 80% random
loss and 4s delay barely register; 300ms WAN-uniform delay is
indistinguishable from 100ms). The real-network questions the sim cannot
answer are covered elsewhere: the UDP datagram-size behavior by the real
2-node `gossip_wire` tests (see ceiling 4 above), absolute per-node HTTP
throughput by Milestone 10. Congestion
coupling (user traffic starving gossip) *is* modeled first-order by the
`congestion` scenario: a total gossip blackout longer than `stale_timeout`
under 2× load admits the full offered load (soft-limit worst case,
offered-bound), and recovers within ~5s of the link draining.
