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

### 2. Node count — control convergence, not gossip membership

**Law: convergence time ≈ 0.7s × node count** at the production gains
(measured: 37s @ 50 nodes, 72s @ 100, 141s @ 200, 278s @ 400, independent of
rate). Equal division hands each node `error / n` of the cluster error while
gains stay fixed, so settling stretches linearly. Pick your ops tolerance
and read off the ceiling: convergence within a minute → ~85 nodes; within
five → ~400. Gain scheduling by fleet size (Milestone 5) is the lever that
would flatten this law. Chitchat membership itself is not the binder at
these sizes (Quickwit operates it at hundreds of nodes).

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

### 4. Scope count — gossip encoding/bandwidth, not CPU

CPU is not the binder: serialization measures ~88 ns/scope per publish and
aggregation ~34 ns per scope-peer per tick (10 peers × 10k scopes ≈ 3.4 ms +
0.9 ms every 500 ms ≈ <1% of a core). The binder is the wire: **~115
bytes/scope, currently shipped as one monolithic JSON value under a single
chitchat key**, so every publish retransmits everything (10k scopes = 1.15
MB per exchange, ~2.3 MB/s per active peer link). Worse, chitchat gossips
over UDP with MTU-bounded messages — whether a megabyte-scale single value
propagates *at all* on a real cluster is unknown and **cannot be answered by
the simulator**; verify on a real 2-node cluster at 10k scopes before
quoting any scope ceiling. Assume O(1–10k) gossiped scopes today. The fixes
are the Milestone 6 items: per-scope keys + compact encoding, then two-tier
coordination (only near-limit scopes gossip at all) for per-user scale.

## Sizing formula (per node)

```
memory  ≈  scopes × ~1 KB                      (limiter + map overhead, idle)
         + accepted_rps × 16 B                 (sliding window, one update_interval deep)
gossip  ≈  gossiped_scopes × 115 B × 2/s       (per active peer link, today's encoding)
CPU     ≈  rps × 30 ns  (decisions)
         + gossiped_scopes × (88 + 34 × peers) ns / 500 ms   (sync loop)
         + HTTP stack (dominant; Milestone 10 measures)
```

Cold-start caveat: buckets initialize at a full cluster-target of tokens, so
a joining node admits a burst of up to `bucket_capacity` requests
immediately (measured ~250 excess requests per join in the autoscale
scenario). Tracked as the cold-start fair-share item in Milestone 5.

## What the simulator deliberately does not model

The gossip transport is an abstract message bus (delay + jitter + loss +
partitions). Not modeled: UDP/MTU fragmentation, chitchat anti-entropy
rounds, kernel queues, packet reordering effects on versioned KV state,
serialization on the wire. This is the right trade for a *soft*, latest-wins,
2 Hz protocol with a 10s staleness horizon: transport pathologies below
~1s are invisible to every mechanism in the system (verified: 80% random
loss and 4s delay barely register; 300ms WAN-uniform delay is
indistinguishable from 100ms). The two real-network questions the sim
cannot answer — the MTU/blob question above and absolute per-node HTTP
throughput — are exactly the Milestone 10 real-cluster items. Congestion
coupling (user traffic starving gossip) *is* modeled first-order by the
`congestion` scenario: a total gossip blackout longer than `stale_timeout`
under 2× load admits the full offered load (soft-limit worst case,
offered-bound), and recovers within ~5s of the link draining.
