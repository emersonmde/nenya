# Tuning Guide

How to configure nenya from things you already know about your system —
your limits, node count, user population, traffic shape, and load-balancer
setup. You should not need to understand the control loop or the tier
state machine to deploy it well; the internal thresholds ship with
simulator-derived defaults and are deliberately not part of this guide
(they are documented in [capacity-model.md](capacity-model.md) for the
curious).

## Start from four numbers

| You know | Set |
|---|---|
| The per-user (or per-client) limit you want to enforce | `NENYA_DEFAULT_TARGET_RATE` — the pattern's `target_rate` is the **cluster-wide** limit per scope, not per node |
| An upstream quota you must stay under (Bedrock TPM, AgentCore TPS, a partner API) | A pattern whose `target_rate` is that quota, with scopes = users. The upstream enforces the hard cap; nenya provides fair division and smoothing under it |
| Your node addresses (or a seed subset) | `NENYA_SEED_NODES` — gossip membership handles the rest; you do not configure node count anywhere |
| How long a user stays "active" between visits | `NENYA_SCOPE_TTL_SECS` (default 60) — memory is `≈ 360 B × users active within this window`, so 1M users active per minute ≈ 360 MB/node |

## What you do NOT need to compensate for

- **Node count**: limits are cluster-wide. Adding nodes does not change
  what a user can do; a user's traffic is capped at the limit whether it
  lands on one node or twenty. Convergence time is flat in fleet size
  (measured at 10/50/100 nodes).
- **Load-balancer policy**: round-robin, random, least-connections, and
  latency-based routing all behave identically (measured). Session
  affinity (sticky sessions, connection reuse) is the *best* case: a
  single-node user is enforced entirely locally with no coordination
  traffic at all.
- **Request concurrency / thread count**: decisions are per-request
  against a shared per-scope bucket; concurrency inside your app doesn't
  change admission math. Size the sidecar's HTTP capacity, not nenya's
  algorithms.
- **Burst allowance**: a user may burst roughly one second's worth of
  their limit through whichever node they hit, then sustain the limit.
  There is no separate burst knob to set.

## What to think about by traffic shape

- **Steady per-user API traffic** (the common case): defaults apply.
  Expect enforcement accuracy within a few percent of the limit; a user
  briefly exceeding the limit while ramping is coordinated within ~1–2
  seconds (soft-limit semantics — see the honesty note below).
- **Very spiky traffic** (cron-driven clients, batch jobs): the
  first-second burst is bounded by `nodes × limit` per user if the spike
  is perfectly spread across nodes. If a downstream dependency cannot
  absorb that for one second, lower the per-user `target_rate` so that
  `nodes × limit` fits, or front the spike with a service-level pattern.
- **Many users near their limit simultaneously** (e.g. a fleet of
  identical bots): each such user costs a gossip entry. The per-node
  publish budget (`NENYA_GOSSIP_BUDGET`, default 1000) caps the total;
  if you expect more than ~1000 concurrently limit-adjacent users per
  node, raise it (cost: ~31 bytes × entries × 2/s per peer link).
- **Millions of mostly-idle users**: this is the design point. Idle
  users cost nothing after the TTL; active-but-modest users cost ~360 B
  and no gossip.

## Timing knobs (rarely needed)

- `NENYA_SYNC_INTERVAL_MS` (default 500): how often nodes exchange
  rates. Lower = faster coordination, more gossip chatter. Keep the
  default unless your limits are so tight that 1–2 s of coordination lag
  matters, in which case also reconsider whether a soft limiter is the
  right tool.
- `NENYA_STALE_TIMEOUT_MS` (default 10 000): how long a silent node's
  last-known rates still count. Lower = faster failover of a crashed
  node's share, higher = more tolerance for network hiccups. Must exceed
  2 × sync interval.
- Control gains (`NENYA_DEFAULT_KP/KI/KD`) and engine
  (`NENYA_DEFAULT_ENGINE`): leave at defaults. They were chosen by
  simulator sweep, and the failure modes of hand-tuning (limit cycles,
  windup) are subtle. The one documented reason to switch engines:
  heavily skewed *promoted* traffic serves better under
  `NENYA_DEFAULT_ENGINE=bayesian` (demand-weighted division).

## Honesty notes (what this system does not give you)

- Limits are **soft**: worst-case overshoot ≈ coordination lag × excess
  demand. An unpromoted user is bounded at `1.25 × limit` cluster-wide;
  a spread user ramping hard can reach `min(offered, nodes × limit)` for
  about one second before coordination engages. If you need
  billing-grade enforcement, put the hard counter at the resource.
- Per-user limits do not protect downstream infrastructure from floods
  spread across *many* users — total admitted traffic scales with user
  count times the per-user limit. Use a service-level pattern (a
  coarser-grained scope) as the aggregate backstop.
