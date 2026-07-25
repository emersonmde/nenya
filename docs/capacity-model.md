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
  limit promotes 17 scopes (uniform routing; 54 sticky) — the gossip
  payload is the hot head, not the population.

### Is promotion still needed after delta sync? (ablation)

The per-scope-key wire format removed the original scope-count binder
(retransmitting every scope on any change), so the question deserved a
before/after test rather than an assumption
(`tests/two_tier_ablation.rs`, 300k scopes, 2 peers, release, seed 42):

| | two-tier (default) | all-hot (ablated) |
|---|---|---|
| RSS | 356 B/scope | 450 B/scope (grows under load) |
| sync tick (apply+collect, /500 ms) | **1.3 ms** | **218 ms** |
| replicated keyspace / joiner catch-up | 0 keys | 300k keys ≈ 5.9 MB |
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

## Two-tier defaults (Milestone 6.4 sweep, seed 42)

Derivation data from `tier_threshold_sweep` (re-run:
`cargo test --all-features --release --test simulation tier_threshold_sweep -- --ignored --nocapture`).

**Promotion-estimator window** — noise vs. detection lag (promote=0.5,
Zipf 100k users, ~10 truly over threshold; negative lag = the 2 rps
pre-ramp phase promoted spuriously):

| window | promoted | ramp promotion lag |
|--------|----------|--------------------|
| 1s | 78 | −29.3s |
| 2s | 37 | −7.7s |
| 3s | 31 | −4.3s |
| 5s | 22 | −2.6s |
| **8s** | **17** | **+1.0s** |
| 12s | 15 | +2.5s |
| 16s | 13 | +2.9s |

8 s is the knee: the first window where sub-threshold traffic never
promotes; wider windows shave 2–4 scopes for 1.5–2 s more lag.

**Promotion threshold** — the expected promoted-set vs. worst-overage knee
**does not exist**: per-user overage is structurally absent at every
threshold (an unpromoted scope is capped at `limit / n` per node, so its
cluster-wide total cannot exceed the limit, and routing skew raises the
hot node's local estimate, promoting *earlier*). The threshold only trades
hot-set size against coordination headroom:

| promote | promoted | worst unpromoted rps (limit 10) | sticky worst served/offered | ramp max 1s |
|---------|----------|-------------------------------|------------------------------|-------------|
| 0.3 | 35 | 1.80 | 0.40 | 22 |
| **0.5** | **17** | **2.53** | **0.40** | **16** |
| 0.8 | 7 | 5.52 | 0.39 | 16 |

Default 0.5 keeps 2× headroom between promotion and the limit; 0.8 halves
the hot set with no measured downside in these scenarios (exposed as
`NENYA_PROMOTE_UTILIZATION` / per-pattern config).

**Demotion threshold** — flap resistance (user parked at the demotion
boundary, 300 s; 3 promotions = one per node = no flap):

| demote | promotions at boundary |
|--------|------------------------|
| 0.15 | 0–3 |
| **0.25** | **3** |
| 0.35 | 12 |
| 0.45 | 15–23 |

0.25 is the highest (fastest hot-set shedding) flap-free value. The
demotion hold (10 s) covers the full information round-trip (sync +
propagation + control interval); the stateright model
(`model_check_tier_state_machine`) proves the hysteresis and
no-flap-under-constant-input properties over all interleavings.

**Tail burst depth + sparse-share floor** (large-cluster follow-up):
splitting a small per-user limit across a large cluster surfaced two
defects and one product tradeoff. Defects (both fixed): with per-node
capacity `share × 1s`, any cluster larger than the per-user rps limit
gave sub-token buckets that could **never admit anything** (and never
promote, since the estimator counts accepts) — and even after promotion,
a one-token adaptive bucket lost ~40% of a Poisson stream to clumping.
The adaptive capacity now floors at 4 tokens (swept 1/2/4/8 → served
0.62/0.84/0.94/0.97 for an 8 rps user on 25 nodes; service-scale
scenarios unchanged; 4 is the knee).

The tradeoff — concentrated-burst tolerance vs. cold-bucket spike — is
the `tail_burst_fraction` knob (per-node tail capacity
`max(share, frac × limit) × 1s`). Depth changes neither long-run
admission (refill stays at the fair share) nor steady per-user overage
(flat at every fraction); it trades how much of a client burst one node
absorbs against the `n × frac × limit` worst-case spike for a
synchronized spread burst:

| frac | 20-req burst via 1 node (limit 10, 10 nodes) | autoscale join overshoot (service L=300, budget <4000) |
|------|------|------|
| 0.0 (share only) | 1/20 | 2452 |
| 0.25 | 2/20 | 2452 |
| **0.5** | **5/20** | **2698** |
| 1.0 | 10/20 | 2793 |

Shipped default 0.5: aligned with the promotion threshold (one node
absorbs bursts up to the utilization level where coordination takes
over; a burst that trips promotion carries its remaining tail tokens
into the promoted limiter rather than being truncated). The direction of
the trade is deliberately toward usability — premature throttling of
legitimate bursts is a chronic, per-customer cost, while the spread-burst
spike is bounded, once-per-refill-period, and the sustained many-scope
flood it hints at is unwinnable by per-user limits regardless (that is
the service-level cap's job). Per-user-focused deployments can set
`NENYA_TAIL_BURST_FRACTION=1.0` for Redis-style full-limit burst
semantics; DDoS-sensitive ones can lower it toward 0.

**Routing strategies — measured, only stickiness matters**: the promotion
estimate assumes uniform routing, so four load-balancer policies were
compared on the same Zipf population (`test_routing_strategies_preserve_two_tier_invariants`,
seed 42, 100k users, 60× a 10 rps limit):

| routing | promoted | worst unpromoted rps | head user rps (offered ~50) | node CV |
|---------|----------|----------------------|------------------------------|---------|
| uniform | 17 | 2.53 | 10.28 | 0.015 |
| round-robin | 17 | 3.33 | 10.40 | 0.002 |
| least-loaded (adverse feedback) | 17 | 3.15 | 10.40 | 0.001 |
| sticky | 42 | 1.22 | 3.52 | 0.017 |

Round-robin is just a lower-variance uniform. Least-loaded — modeled
adversarially as "route every arrival to the node with the lowest
trailing-1s *accepted* rate", i.e. a throttling node's fast 429s attract
more traffic — turns out to act as an equalizer, indistinguishable from
round-robin: per-user capping doesn't create the node-level asymmetry the
feedback loop would need. Static skew is bracketed by the
uniform/sticky endpoints. The only policy that changes outcomes is full
session affinity, and its failure mode is *under-service* (head user
capped at the ~`limit/n` equal-division share, 3.5 of 10 rps), not
overage — the same engine-side property noted below, addressable by
demand-weighted division, not by tier policy.

**Count-min sketch — evaluated and rejected**: a mergeable sketch of tail
rates would answer "approximate cluster rate for any user" at fixed gossip
size. The simulator data shows promotion + per-pattern tail aggregate
already suffices: unpromoted overage is zero in both uniform and sticky
routing, so the sketch's conservative-overestimate property has nothing to
protect. The remaining gap — sticky mid-band users are served ~40% of
offered (capped at the equal share until promotion) — is an *equal
division* property that a sketch cannot fix; demand-weighted division
(the Bayesian engine's niche) is the lever for that. No sketch ships.

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
