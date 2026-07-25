//! In-process simulated cluster: N nodes, a message-bus gossip model, and a
//! virtual clock.
//!
//! Each simulated node owns real `RateLimiter`s (one per scope, constructed
//! exactly as `RateLimitManager` constructs them) and runs the same sync-loop
//! sequence as `gossip::sync::gossip_sync_loop`: refresh local rates → publish
//! → aggregate peer observations with `gossip::aggregate` (the production
//! decay code, not a reimplementation) → apply external rate + live peer
//! count in one pass, zero-resetting scopes with no live peer data.
//!
//! The virtual clock is `start + tick_index × tick`: all limiter interactions
//! go through the explicit-timestamp APIs (`should_throttle_at`,
//! `update_state_at`), so no wall-clock time or sleeps are involved and a
//! 60-second scenario runs in milliseconds.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use crate::engine::{BayesianEngine, EngineKind, HybridEngine, PeerRate, PidEngine};
use crate::gossip::aggregate::{aggregate_peer_rates, PeerObservation};
use crate::gossip::tier::{
    budget_evictions, should_promote, tail_capacity, watch_threshold, DemotionTracker, RateWindow,
    TailScope, TierConfig,
};
use crate::pid_controller::PIDControllerBuilder;
use crate::{RateLimiter, RateLimiterBuilder};

pub use crate::engine::BayesianParams;

use super::rng::SplitMix64;
use super::workload::{ArrivalProcess, PopulationWorkload, Routing, Workload};
use std::collections::HashSet;

/// Message-bus gossip model parameters.
#[derive(Debug, Clone)]
pub struct GossipModel {
    /// Base one-way propagation delay for a published state to reach a peer
    pub delay: Duration,

    /// Uniform jitter in [0, jitter) added to each message's delay (seeded)
    pub jitter: Duration,

    /// Probability that any individual message is lost
    pub loss: f64,
}

impl Default for GossipModel {
    fn default() -> Self {
        GossipModel {
            // Chitchat converges state in roughly one gossip round trip on a
            // LAN; 100ms ± 50ms models same-region propagation without being
            // tied to any measured deployment. Scenarios that study lag
            // sensitivity override these explicitly.
            delay: Duration::from_millis(100),
            jitter: Duration::from_millis(50),
            loss: 0.0,
        }
    }
}

/// Simulated cluster configuration. Defaults mirror the production defaults
/// in `Config::from_env` / `RateLimitManager::create_limiter_from_pattern`
/// so simulator findings transfer: cluster target 300, min = 0.5×target,
/// max = 2×target, gains (0.5, 0.02, 0.08), 1s PID interval, 500ms sync,
/// 10s stale timeout.
#[derive(Debug, Clone)]
pub struct SimConfig {
    pub num_nodes: usize,
    pub cluster_target: f64,
    pub min_rate: f64,
    pub max_rate: f64,
    pub kp: f64,
    pub ki: f64,
    pub kd: f64,
    pub pid_update_interval: Duration,
    pub sync_interval: Duration,
    pub stale_timeout: Duration,

    /// Integral anti-windup clamp as a fraction of the cluster target
    /// (`None` = unbounded accumulation)
    pub error_limit_frac: Option<f64>,

    /// Control engine to run on every node (explicit config, as in
    /// production)
    pub engine: EngineKind,

    /// Adaptive-window floor for the rate estimator (`None` = library
    /// default; `Some(0)` = pure fixed window, the pre-Milestone-5 behavior)
    pub min_window_samples: Option<usize>,

    /// Initial token fill fraction (`None` = library default, full bucket)
    pub initial_tokens_frac: Option<f64>,

    /// Explicit static bucket capacity (`None` = library default, which is
    /// the adaptive `refill × 1s` allowance; set `Some(target)` to
    /// reproduce the pre-Milestone-5 static cluster-target bucket)
    pub bucket_capacity: Option<f64>,

    /// Adaptive burst allowance override: bucket capacity tracks
    /// `refill × seconds` after each control update
    pub bucket_burst_seconds: Option<f64>,

    /// Estimator parameter overrides for the bayesian/hybrid engines.
    /// `None` uses the engine-appropriate simulator-derived default
    /// (`BayesianParams::default()` / `BayesianParams::hybrid_default()`).
    /// The `stale_timeout` field is always overridden with this config's
    /// `stale_timeout` at limiter construction so the two horizons agree.
    pub estimator: Option<BayesianParams>,

    /// Simulation tick; all activity is quantized to this
    pub tick: Duration,

    pub gossip: GossipModel,

    /// Nodes that start down (for join scenarios)
    pub initially_down: Vec<usize>,

    /// Two-tier promotion/demotion policy (production defaults; see
    /// `gossip::tier`). Scopes start in the compact tail tier and are
    /// promoted into gossip coordination by the same policy code the
    /// server runs.
    pub tier: TierConfig,

    /// Idle-scope TTL (mirrors the production default; sweeps every TTL/2)
    pub scope_ttl: Duration,
}

impl Default for SimConfig {
    fn default() -> Self {
        let cluster_target = 300.0;
        SimConfig {
            num_nodes: 3,
            cluster_target,
            min_rate: cluster_target * 0.5,
            max_rate: cluster_target * 2.0,
            kp: 0.5,
            ki: 0.02,
            kd: 0.08,
            pid_update_interval: Duration::from_secs(1),
            sync_interval: Duration::from_millis(500),
            stale_timeout: Duration::from_secs(10),
            // Production default (see ScopePattern::get_error_limit);
            // derived from the Milestone 4 scenario-matrix sweep
            error_limit_frac: Some(0.2),
            engine: EngineKind::Pid,
            min_window_samples: None,
            initial_tokens_frac: None,
            bucket_capacity: None,
            bucket_burst_seconds: None,
            estimator: None,
            tick: Duration::from_millis(10),
            gossip: GossipModel::default(),
            initially_down: Vec::new(),
            tier: TierConfig::default(),
            scope_ttl: crate::gossip::tier::DEFAULT_SCOPE_TTL,
        }
    }
}

impl SimConfig {
    /// Set the cluster target and derive min/max the way the server's
    /// default scope pattern does (0.5× and 2×).
    pub fn with_cluster_target(mut self, target: f64) -> Self {
        self.cluster_target = target;
        self.min_rate = target * 0.5;
        self.max_rate = target * 2.0;
        self
    }
}

/// A timeline event applied mid-run.
#[derive(Debug, Clone)]
pub enum SimEvent {
    /// Node crashes: stops receiving traffic, publishing, and processing.
    /// Its state is lost (a restart is a fresh join).
    NodeDown(usize),

    /// Node (re)starts with fresh limiter state and empty peer records
    NodeUp(usize),

    /// Partition into groups; messages cross groups are dropped at send time.
    /// Nodes not listed form their own implicit group.
    Partition(Vec<Vec<usize>>),

    /// Remove all partitions
    Heal,

    /// Change the gossip message-loss probability mid-run. Models network
    /// congestion coupling: when user traffic saturates the links, gossip
    /// degrades exactly when coordination matters most (set 1.0 for a full
    /// gossip blackout, back to the baseline to model the queue draining).
    GossipLoss(f64),
}

impl SimEvent {
    pub fn label(&self) -> String {
        match self {
            SimEvent::NodeDown(i) => format!("node{}_down", i),
            SimEvent::NodeUp(i) => format!("node{}_up", i),
            SimEvent::Partition(groups) => format!("partition_{:?}", groups),
            SimEvent::Heal => "heal".to_string(),
            SimEvent::GossipLoss(p) => format!("gossip_loss_{:.0}pct", p * 100.0),
        }
    }
}

struct PeerRecord {
    rates: HashMap<String, f64>,
    /// Peer's tail aggregate (summed unpromoted-scope rate; the sim's
    /// single implicit pattern is `*`)
    tail_rate: f64,
    /// Simulated time the record last changed (age-at-receipt, mirroring
    /// `GossipManager`'s local-monotonic-clock bookkeeping)
    received_at: Duration,
}

/// One scope's tiered state on a simulated node (mirrors the server's
/// `ScopeEntry`, minus the non-distributed `Local` variant — every sim
/// scope is distributed)
enum SimScope {
    Tail {
        tail: TailScope,
    },
    Hot {
        /// Boxed so the enum stays tail-sized (mirrors the server's
        /// `ScopeEntry`; per-user scenarios hold 10⁵+ entries per node)
        limiter: Box<RateLimiter<f64>>,
        demotion: DemotionTracker,
    },
}

struct SimNode {
    up: bool,
    scopes: BTreeMap<String, SimScope>,
    records: BTreeMap<usize, PeerRecord>,
    /// Node-level live-peer count from the last sync pass (tail scopes
    /// derive their equal share from this)
    live_peers: usize,
    /// Number of hot-tier scopes (bounded by the gossip budget at each sync)
    hot_count: usize,
    /// Promotion admission floor while at the gossip budget (mirrors
    /// `RateLimitManager::promotion_floor`)
    promotion_floor: f64,
    /// Tail aggregate for the node's single implicit pattern (summed
    /// accepted rate of unpromoted scopes, maintained on the admit path)
    tail_window: RateWindow,
    /// Trailing accepted rate across all scopes (LeastLoaded routing input)
    accept_window: RateWindow,
    /// Tail scopes currently publishing their rate (locally warm — above
    /// the watch watermark); pruned each sync
    watched: std::collections::BTreeSet<String>,
    /// Sim time of the last idle-scope TTL sweep
    last_ttl_sweep: Duration,
    /// Fractional-arrival accumulator per scope (deterministic arrivals)
    accum: BTreeMap<String, f64>,
    rng: SplitMix64,
}

struct Delivery {
    to: usize,
    from: usize,
    rates: HashMap<String, f64>,
    tail_rate: f64,
}

/// Per-tick request counters.
#[derive(Debug, Clone)]
pub struct TickCounts {
    pub offered: u64,
    pub accepted: u64,
    pub per_node_accepted: Vec<u64>,
}

/// Runtime state of one heavy-tailed user population: the workload, its
/// precomputed Zipf CDF, and a dedicated arrival RNG.
struct PopState {
    wl: PopulationWorkload,
    /// Cumulative rank-frequency distribution (`cdf[r]` = P(rank ≤ r))
    cdf: Vec<f64>,
    rng: SplitMix64,
    /// Round-robin cursor (Routing::RoundRobin)
    rr_next: usize,
}

impl PopState {
    fn new(wl: PopulationWorkload, rng: SplitMix64) -> Self {
        let mut cdf = Vec::with_capacity(wl.users);
        let mut total = 0.0;
        for rank in 0..wl.users {
            total += 1.0 / ((rank + 1) as f64).powf(wl.zipf_s);
            cdf.push(total);
        }
        for c in cdf.iter_mut() {
            *c /= total;
        }
        PopState {
            wl,
            cdf,
            rng,
            rr_next: 0,
        }
    }

    /// Sample a user rank from the Zipf CDF (binary search).
    fn sample_user(&mut self) -> usize {
        let u = self.rng.next_f64();
        self.cdf.partition_point(|&c| c < u).min(self.wl.users - 1)
    }
}

pub struct SimCluster {
    cfg: SimConfig,
    workloads: Vec<Workload>,
    populations: Vec<PopState>,
    nodes: Vec<SimNode>,
    /// Cluster-wide per-scope request counters (offered, accepted) —
    /// per-user assertions in the Milestone 6 scenarios read these
    scope_offered: HashMap<String, u64>,
    scope_accepted: HashMap<String, u64>,
    /// Scopes that were ever promoted to the hot tier on any node
    ever_hot: HashSet<String>,
    /// Promotion events per scope across all nodes (local + peer-triggered)
    /// — the flap metric for the demotion-hysteresis sweep
    promotion_count: HashMap<String, u32>,
    /// High-water mark of each node's hot-tier size (gossip payload bound)
    max_hot: Vec<usize>,
    /// Partition group per node; same group = connected
    group: Vec<usize>,
    /// Messages in flight, keyed by delivery tick
    inbox: BTreeMap<u64, Vec<Delivery>>,
    /// Virtual clock base; only ever offset by whole ticks
    start: Instant,
    tick_index: u64,
    sync_ticks: u64,
    /// Publish phase offset per node so nodes don't gossip in lockstep
    phase: Vec<u64>,
    bus_rng: SplitMix64,
}

impl SimCluster {
    pub fn new(cfg: SimConfig, workloads: Vec<Workload>, seed: u64) -> Self {
        Self::with_populations(cfg, workloads, Vec::new(), seed)
    }

    pub fn with_populations(
        cfg: SimConfig,
        workloads: Vec<Workload>,
        populations: Vec<PopulationWorkload>,
        seed: u64,
    ) -> Self {
        assert!(cfg.num_nodes > 0, "cluster needs at least one node");
        assert!(!cfg.tick.is_zero(), "tick must be positive");

        let mut root_rng = SplitMix64::new(seed);
        let bus_rng = root_rng.fork();

        let start = Instant::now();
        let sync_ticks = (cfg.sync_interval.as_nanos() / cfg.tick.as_nanos()).max(1) as u64;

        let mut nodes = Vec::with_capacity(cfg.num_nodes);
        for i in 0..cfg.num_nodes {
            let up = !cfg.initially_down.contains(&i);
            // Scopes are auto-created in the tail tier on first arrival
            // (mirroring the server's auto-creation), not pre-allocated
            let node = SimNode {
                up,
                scopes: BTreeMap::new(),
                records: BTreeMap::new(),
                live_peers: 0,
                hot_count: 0,
                promotion_floor: 0.0,
                tail_window: RateWindow::new(start),
                accept_window: RateWindow::new(start),
                watched: std::collections::BTreeSet::new(),
                last_ttl_sweep: Duration::ZERO,
                accum: BTreeMap::new(),
                rng: root_rng.fork(),
            };
            nodes.push(node);
        }

        // Spread publish ticks evenly across the sync interval
        let phase: Vec<u64> = (0..cfg.num_nodes as u64)
            .map(|i| i * sync_ticks / cfg.num_nodes as u64)
            .collect();

        let populations: Vec<PopState> = populations
            .into_iter()
            .map(|wl| PopState::new(wl, root_rng.fork()))
            .collect();

        SimCluster {
            group: vec![0; cfg.num_nodes],
            max_hot: vec![0; cfg.num_nodes],
            nodes,
            workloads,
            populations,
            scope_offered: HashMap::new(),
            scope_accepted: HashMap::new(),
            ever_hot: HashSet::new(),
            promotion_count: HashMap::new(),
            inbox: BTreeMap::new(),
            start,
            tick_index: 0,
            sync_ticks,
            phase,
            bus_rng,
            cfg,
        }
    }

    pub fn config(&self) -> &SimConfig {
        &self.cfg
    }

    /// Simulated time elapsed since the run started.
    pub fn sim_time(&self) -> Duration {
        self.cfg.tick * self.tick_index as u32
    }

    fn now(&self) -> Instant {
        self.start + self.sim_time()
    }

    /// Total offered rate across workloads and populations at simulated
    /// time `t`.
    pub fn offered_rate_at(&self, t: Duration) -> f64 {
        self.workloads
            .iter()
            .map(|w| w.pattern.rate_at(t))
            .chain(self.populations.iter().map(|p| p.wl.pattern.rate_at(t)))
            .sum()
    }

    /// Cluster-wide (offered, accepted) request totals for a scope.
    pub fn scope_counts(&self, scope: &str) -> (u64, u64) {
        (
            self.scope_offered.get(scope).copied().unwrap_or(0),
            self.scope_accepted.get(scope).copied().unwrap_or(0),
        )
    }

    /// All scopes with their cluster-wide (offered, accepted) totals.
    pub fn all_scope_counts(&self) -> impl Iterator<Item = (&String, u64, u64)> {
        self.scope_offered.iter().map(|(scope, &offered)| {
            (
                scope,
                offered,
                self.scope_accepted.get(scope).copied().unwrap_or(0),
            )
        })
    }

    /// Was this scope ever promoted to the hot tier on any node?
    pub fn was_ever_hot(&self, scope: &str) -> bool {
        self.ever_hot.contains(scope)
    }

    /// Number of scopes ever promoted on any node.
    pub fn num_ever_hot(&self) -> usize {
        self.ever_hot.len()
    }

    /// Promotion events for a scope across all nodes (re-promotions count).
    pub fn promotions_of(&self, scope: &str) -> u32 {
        self.promotion_count.get(scope).copied().unwrap_or(0)
    }

    /// High-water mark of a node's hot-tier size (bounds its gossip
    /// payload: at most this many scope keys were ever published).
    pub fn max_hot_scopes(&self, node: usize) -> usize {
        self.max_hot[node]
    }

    pub fn node_is_up(&self, i: usize) -> bool {
        self.nodes[i].up
    }

    /// Apply a timeline event at the current simulated time.
    pub fn apply_event(&mut self, event: &SimEvent) {
        match event {
            SimEvent::NodeDown(i) => {
                let now = self.now();
                let node = &mut self.nodes[*i];
                node.up = false;
                // Crash loses all state
                node.scopes.clear();
                node.records.clear();
                node.accum.clear();
                node.live_peers = 0;
                node.hot_count = 0;
                node.promotion_floor = 0.0;
                node.watched.clear();
                node.tail_window = RateWindow::new(now);
            }
            SimEvent::NodeUp(i) => {
                let now = self.now();
                let node = &mut self.nodes[*i];
                node.up = true;
                // Fresh join: scopes re-created in the tail tier on first
                // arrival, exactly like a restarted server process
                node.scopes.clear();
                node.records.clear();
                node.accum.clear();
                node.live_peers = 0;
                node.hot_count = 0;
                node.promotion_floor = 0.0;
                node.watched.clear();
                node.tail_window = RateWindow::new(now);
            }
            SimEvent::Partition(groups) => {
                // Group 0 is the implicit group for unlisted nodes
                for g in self.group.iter_mut() {
                    *g = 0;
                }
                for (idx, members) in groups.iter().enumerate() {
                    for &m in members {
                        self.group[m] = idx + 1;
                    }
                }
            }
            SimEvent::Heal => {
                for g in self.group.iter_mut() {
                    *g = 0;
                }
            }
            SimEvent::GossipLoss(p) => {
                self.cfg.gossip.loss = p.clamp(0.0, 1.0);
            }
        }
    }

    /// Advance the simulation by one tick: deliver due gossip, run per-node
    /// sync loops at their boundaries, then dispatch workload arrivals.
    pub fn run_tick(&mut self) -> TickCounts {
        let now = self.now();
        let t = self.sim_time();

        // 1. Deliver gossip messages due this tick
        if let Some(deliveries) = self.inbox.remove(&self.tick_index) {
            for d in deliveries {
                let node = &mut self.nodes[d.to];
                // A down node misses messages entirely (they are not queued
                // for its return — a restart is a fresh join)
                if node.up {
                    node.records.insert(
                        d.from,
                        PeerRecord {
                            rates: d.rates,
                            tail_rate: d.tail_rate,
                            received_at: t,
                        },
                    );
                }
            }
        }

        // 2. Per-node gossip sync at staggered boundaries
        for i in 0..self.cfg.num_nodes {
            if self.nodes[i].up && (self.tick_index + self.phase[i]).is_multiple_of(self.sync_ticks)
            {
                self.sync_node(i, now, t);
            }
        }

        // 3. Workload arrivals. Plan per-node arrival means first (immutable
        // pass), then dispatch (mutable pass).
        let dt = self.cfg.tick.as_secs_f64();
        let mut plans: Vec<(String, ArrivalProcess, Vec<f64>)> = Vec::new();
        for w in &self.workloads {
            let rate = w.pattern.rate_at(t);
            let weights: Vec<f64> = (0..self.cfg.num_nodes)
                .map(|i| {
                    if !self.nodes[i].up {
                        return 0.0;
                    }
                    match &w.node_weights {
                        Some(ws) => ws.get(i).copied().unwrap_or(0.0),
                        None => 1.0,
                    }
                })
                .collect();
            let total: f64 = weights.iter().sum();
            if total <= 0.0 {
                continue;
            }
            let lambdas = weights
                .iter()
                .map(|weight| rate * weight / total * dt)
                .collect();
            plans.push((w.scope.clone(), w.arrival, lambdas));
        }

        let mut counts = TickCounts {
            offered: 0,
            accepted: 0,
            per_node_accepted: vec![0; self.cfg.num_nodes],
        };

        for (scope, arrival, lambdas) in plans {
            for (i, &lambda) in lambdas.iter().enumerate() {
                if lambda <= 0.0 {
                    continue;
                }
                let node = &mut self.nodes[i];
                let n_arrivals = match arrival {
                    ArrivalProcess::Poisson => node.rng.poisson(lambda),
                    ArrivalProcess::Deterministic => {
                        let acc = node.accum.entry(scope.clone()).or_insert(0.0);
                        *acc += lambda;
                        let whole = acc.floor();
                        *acc -= whole;
                        whole as u64
                    }
                };
                if n_arrivals == 0 {
                    continue;
                }
                for _ in 0..n_arrivals {
                    counts.offered += 1;
                    *self.scope_offered.entry(scope.clone()).or_default() += 1;
                    let outcome = admit_one(&self.cfg, node, &scope, now);
                    if outcome.admitted {
                        counts.accepted += 1;
                        counts.per_node_accepted[i] += 1;
                        *self.scope_accepted.entry(scope.clone()).or_default() += 1;
                        node.accept_window.record(now);
                    }
                }
            }
        }

        // 4. Population arrivals (heavy-tailed per-user traffic): draw the
        // total Poisson arrival count from the offered curve, then assign
        // each arrival a user by the Zipf law and a node by the routing
        // policy
        let up_nodes: Vec<usize> = (0..self.cfg.num_nodes)
            .filter(|&i| self.nodes[i].up)
            .collect();
        if !up_nodes.is_empty() {
            for p in 0..self.populations.len() {
                let lambda = self.populations[p].wl.pattern.rate_at(t) * dt;
                if lambda <= 0.0 {
                    continue;
                }
                let n_arrivals = self.populations[p].rng.poisson(lambda);
                for _ in 0..n_arrivals {
                    let pop = &mut self.populations[p];
                    let user = pop.sample_user();
                    let scope = format!("{}{}", pop.wl.prefix, user);
                    let node_idx = match pop.wl.routing {
                        Routing::Uniform => {
                            up_nodes[(pop.rng.next_u64() % up_nodes.len() as u64) as usize]
                        }
                        Routing::Sticky => {
                            // Preferred node by user identity; fall forward
                            // to the next up node when it's down
                            let preferred = user % self.cfg.num_nodes;
                            (0..self.cfg.num_nodes)
                                .map(|k| (preferred + k) % self.cfg.num_nodes)
                                .find(|&i| self.nodes[i].up)
                                .expect("at least one up node")
                        }
                        Routing::RoundRobin => {
                            let idx = up_nodes[pop.rr_next % up_nodes.len()];
                            pop.rr_next = (pop.rr_next + 1) % up_nodes.len();
                            idx
                        }
                        Routing::LeastLoaded => *up_nodes
                            .iter()
                            .min_by(|&&a, &&b| {
                                self.nodes[a]
                                    .accept_window
                                    .rate_at(now)
                                    .partial_cmp(&self.nodes[b].accept_window.rate_at(now))
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })
                            .expect("at least one up node"),
                    };
                    counts.offered += 1;
                    *self.scope_offered.entry(scope.clone()).or_default() += 1;
                    let node = &mut self.nodes[node_idx];
                    let outcome = admit_one(&self.cfg, node, &scope, now);
                    if outcome.admitted {
                        counts.accepted += 1;
                        counts.per_node_accepted[node_idx] += 1;
                        *self.scope_accepted.entry(scope).or_default() += 1;
                        self.nodes[node_idx].accept_window.record(now);
                    }
                }
            }
        }

        // High-water mark of per-node hot-tier size (gossip payload bound)
        for i in 0..self.cfg.num_nodes {
            self.max_hot[i] = self.max_hot[i].max(self.nodes[i].hot_count);
        }

        self.tick_index += 1;
        counts
    }

    /// One node's sync-loop pass, mirroring `gossip_sync_loop` step for
    /// step: aggregate peer observations → peer-triggered promotion → apply
    /// observations + demotion hysteresis + gossip-budget eviction →
    /// refresh and publish hot-tier rates.
    fn sync_node(&mut self, i: usize, now: Instant, t: Duration) {
        let tier_cfg = self.cfg.tier;
        let limit = self.cfg.cluster_target;

        // Step 1: aggregate peer observations with the production decay code
        let observations: Vec<PeerObservation> = self.nodes[i]
            .records
            .iter()
            .map(|(peer, rec)| PeerObservation {
                node_id: peer.to_string(),
                age: t.saturating_sub(rec.received_at),
                scope_rates: rec.rates.clone(),
                tail_rates: HashMap::from([("*".to_string(), rec.tail_rate)]),
            })
            .collect();
        let aggregated = aggregate_peer_rates(
            &observations,
            self.cfg.sync_interval,
            self.cfg.stale_timeout,
        );

        let local_rates = {
            let node = &mut self.nodes[i];
            node.live_peers = aggregated.live_peers;
            let share = limit / (1 + node.live_peers) as f64;

            // Step 2: evidence-based promotion — the observed cluster rate
            // (local estimate + staleness-weighted peer sum) crosses the
            // promotion threshold with nonzero peer evidence. A scope with
            // no peer evidence stays tail no matter how hot locally: one
            // full-limit bucket already caps it.
            for scope in aggregated.scope_rates.keys() {
                if let Some(SimScope::Tail { tail }) = node.scopes.get_mut(scope) {
                    let peer_rate = aggregated.scope_rates.get(scope).copied().unwrap_or(0.0);
                    let local_rate = tail.local_rate_at(now);
                    if !should_promote(local_rate, peer_rate, limit, &tier_cfg) {
                        continue;
                    }
                    let combined = local_rate + peer_rate;
                    if node.hot_count >= tier_cfg.gossip_budget
                        && combined / limit < node.promotion_floor
                    {
                        continue;
                    }
                    let tokens = tail.tokens();
                    // Refill continuity: start at the measured local rate
                    // (at least the equal share); the engine adjusts from
                    // there
                    let refill0 = local_rate.max(share);
                    let limiter = make_hot_limiter(&self.cfg, now, refill0, tokens);
                    node.scopes.insert(
                        scope.clone(),
                        SimScope::Hot {
                            limiter: Box::new(limiter),
                            demotion: DemotionTracker::default(),
                        },
                    );
                    node.watched.remove(scope);
                    node.hot_count += 1;
                    self.ever_hot.insert(scope.clone());
                    *self.promotion_count.entry(scope.clone()).or_default() += 1;
                }
            }

            // Step 3: apply external rates, live peer count, and per-peer
            // observations to hot scopes (scopes absent from the aggregate
            // are explicitly zeroed), feeding the demotion hysteresis
            let mut demote: Vec<String> = Vec::new();
            let mut utilizations: Vec<(String, f64)> = Vec::new();
            for (scope, entry) in node.scopes.iter_mut() {
                let SimScope::Hot { limiter, demotion } = entry else {
                    continue;
                };
                let external = aggregated.scope_rates.get(scope).copied().unwrap_or(0.0);
                limiter.set_external_accepted_request_rate(external);
                limiter.set_num_peers(aggregated.live_peers);
                let obs: Vec<PeerRate<f64>> = observations
                    .iter()
                    .filter_map(|o| {
                        o.scope_rates.get(scope).map(|rate| PeerRate {
                            id: o.node_id.clone(),
                            rate: *rate,
                            age: o.age,
                        })
                    })
                    .collect();
                limiter.set_peer_observations(obs);

                let cluster_rate = limiter.local_accepted_request_rate() + external;
                if demotion.observe(cluster_rate, limit, now, &tier_cfg) {
                    demote.push(scope.clone());
                } else {
                    utilizations.push((scope.clone(), cluster_rate / limit));
                }
            }
            for scope in &demote {
                demote_sim_scope(node, scope, limit, now, tier_cfg.estimator_window);
            }

            // Step 4: gossip budget — evict lowest-utilization hot scopes
            if node.hot_count > tier_cfg.gossip_budget {
                let evictions = budget_evictions(utilizations.clone(), tier_cfg.gossip_budget);
                let evicted: std::collections::HashSet<&String> = evictions.iter().collect();
                utilizations.retain(|(name, _)| !evicted.contains(name));
                for scope in &evictions {
                    demote_sim_scope(node, scope, limit, now, tier_cfg.estimator_window);
                }
            }
            node.promotion_floor = if node.hot_count >= tier_cfg.gossip_budget {
                let floor = utilizations
                    .iter()
                    .map(|(_, u)| *u)
                    .fold(f64::INFINITY, f64::min);
                if floor.is_finite() {
                    floor.max(0.0)
                } else {
                    0.0
                }
            } else {
                0.0
            };

            // Step 4b: TTL sweep — evict idle tail scopes (behaviorally
            // lossless past the estimator window; hot scopes demote first)
            if t.saturating_sub(node.last_ttl_sweep) >= self.cfg.scope_ttl / 2 {
                node.last_ttl_sweep = t;
                let ttl = self.cfg.scope_ttl;
                node.scopes.retain(|_, entry| match entry {
                    SimScope::Tail { tail } => {
                        now.saturating_duration_since(tail.last_activity()) < ttl
                    }
                    SimScope::Hot { .. } => true,
                });
            }

            // Step 5: refresh and collect the publish set — hot scopes
            // plus watched tail scopes (locally warm; their rates are the
            // evidence peers promote on). Watched entries that cooled
            // below the watermark or left the tail tier are pruned.
            let mut local_rates = HashMap::new();
            for (scope, entry) in node.scopes.iter_mut() {
                if let SimScope::Hot { limiter, .. } = entry {
                    limiter.update_state_at(now);
                    local_rates.insert(scope.clone(), limiter.local_accepted_request_rate());
                }
            }
            let watch_floor = watch_threshold(limit, 1 + node.live_peers, &tier_cfg);
            let scopes = &node.scopes;
            node.watched.retain(|scope| match scopes.get(scope) {
                Some(SimScope::Tail { tail }) => tail.local_rate_at(now) >= watch_floor,
                _ => false,
            });
            for scope in &node.watched {
                if let Some(SimScope::Tail { tail }) = node.scopes.get(scope) {
                    local_rates.insert(scope.clone(), tail.local_rate_at(now));
                }
            }
            // Gossip budget also caps the published (hot + watched) set
            if local_rates.len() > tier_cfg.gossip_budget {
                let ranked: Vec<(String, f64)> = local_rates
                    .iter()
                    .map(|(scope, rate)| (scope.clone(), rate / limit))
                    .collect();
                for scope in budget_evictions(ranked, tier_cfg.gossip_budget) {
                    local_rates.remove(&scope);
                    node.watched.remove(&scope);
                }
            }
            (local_rates, node.tail_window.rate(now))
        };
        let (local_rates, local_tail_rate) = local_rates;

        // Step 6: publish to every reachable peer through the message bus.
        // An empty rate map is still published — membership liveness rides
        // on the periodic exchange regardless of hot-scope count.
        for j in 0..self.cfg.num_nodes {
            if j == i {
                continue;
            }
            if self.group[i] != self.group[j] {
                continue; // partitioned at send time
            }
            if self.cfg.gossip.loss > 0.0 && self.bus_rng.next_f64() < self.cfg.gossip.loss {
                continue; // lost
            }
            let jitter = self.cfg.gossip.jitter.mul_f64(self.bus_rng.next_f64());
            let delay = self.cfg.gossip.delay + jitter;
            let delay_ticks = (delay.as_nanos() / self.cfg.tick.as_nanos()).max(1) as u64;
            self.inbox
                .entry(self.tick_index + delay_ticks)
                .or_default()
                .push(Delivery {
                    to: j,
                    from: i,
                    rates: local_rates.clone(),
                    tail_rate: local_tail_rate,
                });
        }
    }

    /// This node's tail aggregate (summed accepted rate of unpromoted
    /// scopes), read-only.
    pub fn tail_rate(&self, node: usize, t: Duration) -> f64 {
        self.nodes[node].tail_window.rate_at(self.start + t)
    }

    /// Number of hot-tier (gossiped) scopes on a node.
    pub fn hot_scopes(&self, node: usize) -> usize {
        self.nodes[node].hot_count
    }

    /// Coordination tier of a scope on a node (`"tail"`, `"hot"`, or `None`
    /// if the scope hasn't been created yet).
    pub fn scope_tier(&self, node: usize, scope: &str) -> Option<&'static str> {
        Some(match self.nodes[node].scopes.get(scope)? {
            SimScope::Tail { .. } => "tail",
            SimScope::Hot { .. } => "hot",
        })
    }
}

/// Outcome of one admission attempt.
struct AdmitOutcome {
    admitted: bool,
}

/// Admit one request on a node through its tiered scope entry,
/// auto-creating the scope in the tail tier, exactly as the server's
/// `RateLimitManager::should_throttle_at` does. Tail scopes are enforced
/// at the FULL limit; promotion is evidence-based and happens only in the
/// sync pass (it needs peer observations).
fn admit_one(cfg: &SimConfig, node: &mut SimNode, scope: &str, now: Instant) -> AdmitOutcome {
    let limit = cfg.cluster_target;
    let num_nodes = 1 + node.live_peers;

    if !node.scopes.contains_key(scope) {
        node.scopes.insert(
            scope.to_string(),
            SimScope::Tail {
                tail: TailScope::new(now, tail_capacity(limit), cfg.tier.estimator_window),
            },
        );
    }

    let admitted = match node.scopes.get_mut(scope).expect("just inserted") {
        SimScope::Tail { tail } => {
            let admitted = tail.try_admit(now, limit, tail_capacity(limit));
            if admitted {
                node.tail_window.record(now);
                // Watch: a locally-warm tail scope publishes its rate so
                // spread activity becomes visible cluster-wide
                if tail.local_rate(now) >= watch_threshold(limit, num_nodes, &cfg.tier) {
                    node.watched.insert(scope.to_string());
                }
            }
            admitted
        }
        SimScope::Hot { limiter, .. } => !limiter.should_throttle_at(now),
    };
    AdmitOutcome { admitted }
}

/// Demote a hot sim scope back to the tail tier, carrying its token balance
fn demote_sim_scope(
    node: &mut SimNode,
    scope: &str,
    limit: f64,
    now: Instant,
    estimator_window: Duration,
) {
    let Some(SimScope::Hot { limiter, .. }) = node.scopes.get(scope) else {
        return;
    };
    let tokens = limiter.tokens().min(tail_capacity(limit));
    node.scopes.insert(
        scope.to_string(),
        SimScope::Tail {
            tail: TailScope::with_tokens(now, tokens, estimator_window),
        },
    );
    node.hot_count -= 1;
}

/// Build the full limiter for a scope promoted into the hot tier. Mirrors
/// `RateLimitManager::create_promoted_limiter`: same engine construction as
/// production, refill seeded at the already-enforced equal share, tail
/// tokens carried over.
fn make_hot_limiter(
    cfg: &SimConfig,
    now: Instant,
    share: f64,
    tail_tokens: f64,
) -> RateLimiter<f64> {
    let mut pid_builder = PIDControllerBuilder::new(cfg.cluster_target)
        .kp(cfg.kp)
        .ki(cfg.ki)
        .kd(cfg.kd);
    if let Some(frac) = cfg.error_limit_frac {
        pid_builder = pid_builder.error_limit(cfg.cluster_target * frac);
    }
    let pid = pid_builder.build();

    let engine_default = match cfg.engine {
        EngineKind::Hybrid => BayesianParams::hybrid_default(),
        _ => BayesianParams::default(),
    };
    let estimator_params = BayesianParams {
        stale_timeout: cfg.stale_timeout,
        ..cfg.estimator.unwrap_or(engine_default)
    };

    // Mirror RateLimitManager::create_promoted_limiter: carry the tail
    // bucket's depth (floored at one token) so promotion mid-burst
    // doesn't truncate the burst
    let carried_capacity = tail_tokens.max(share).max(1.0);
    let mut builder = RateLimiterBuilder::new(cfg.cluster_target)
        .cluster_target(cfg.cluster_target)
        .min_rate(cfg.min_rate)
        .max_rate(cfg.max_rate)
        .update_interval(cfg.pid_update_interval)
        .initial_timestamp(now)
        .initial_refill_rate(share)
        .initial_capacity(carried_capacity);
    if let Some(k) = cfg.min_window_samples {
        builder = builder.min_window_samples(k);
    }
    if let Some(cap) = cfg.bucket_capacity {
        builder = builder.bucket_capacity(cap);
    }
    if let Some(secs) = cfg.bucket_burst_seconds {
        builder = builder.bucket_burst_seconds(secs);
    }
    // Config override wins, as elsewhere
    let frac = match cfg.initial_tokens_frac {
        Some(f) => Some(f),
        None => {
            let capacity = cfg.bucket_capacity.unwrap_or(carried_capacity);
            if capacity > 0.0 {
                Some((tail_tokens / capacity).clamp(0.0, 1.0))
            } else {
                None
            }
        }
    };
    if let Some(f) = frac {
        builder = builder.initial_tokens_frac(f);
    }

    match cfg.engine {
        EngineKind::Pid => {
            builder.engine(PidEngine::new(pid).with_staleness(cfg.sync_interval, cfg.stale_timeout))
        }
        EngineKind::Bayesian => builder.engine(BayesianEngine::new(estimator_params)),
        EngineKind::Hybrid => builder.engine(HybridEngine::new(pid, estimator_params)),
    }
    .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::workload::LoadPattern;

    fn constant_workload(rate: f64) -> Vec<Workload> {
        vec![Workload::new("test", LoadPattern::Constant { rate })
            .arrival(ArrivalProcess::Deterministic)]
    }

    fn hot_limiter<'a>(cluster: &'a SimCluster, node: usize, scope: &str) -> &'a RateLimiter<f64> {
        match cluster.nodes[node].scopes.get(scope) {
            Some(SimScope::Hot { limiter, .. }) => limiter,
            other => panic!(
                "scope {:?} on node {} expected hot, got {}",
                scope,
                node,
                match other {
                    Some(SimScope::Tail { .. }) => "tail",
                    _ => "missing",
                }
            ),
        }
    }

    #[test]
    fn test_gossip_reaches_peers() {
        let cfg = SimConfig::default();
        let mut cluster = SimCluster::new(cfg, constant_workload(600.0), 1);
        // Run 5 simulated seconds: plenty for promotion + publish + delivery
        for _ in 0..500 {
            cluster.run_tick();
        }
        for i in 0..3 {
            assert_eq!(
                cluster.nodes[i].records.len(),
                2,
                "node {} should have records from both peers",
                i
            );
            assert_eq!(
                cluster.scope_tier(i, "test"),
                Some("hot"),
                "an over-limit scope must promote on node {}",
                i
            );
            let limiter = hot_limiter(&cluster, i, "test");
            assert_eq!(limiter.num_peers(), 2);
            assert!(
                limiter.external_accepted_request_rate() > 0.0,
                "node {} should see external load",
                i
            );
        }
    }

    #[test]
    fn test_partition_blocks_messages() {
        let cfg = SimConfig::default();
        let mut cluster = SimCluster::new(cfg, constant_workload(600.0), 1);
        for _ in 0..300 {
            cluster.run_tick();
        }
        cluster.apply_event(&SimEvent::Partition(vec![vec![0], vec![1, 2]]));
        // Run past stale_timeout (10s) so pre-partition records fully decay
        for _ in 0..1200 {
            cluster.run_tick();
        }
        assert_eq!(
            hot_limiter(&cluster, 0, "test").num_peers(),
            0,
            "isolated node should see no live peers after stale_timeout"
        );
        assert_eq!(
            hot_limiter(&cluster, 1, "test").num_peers(),
            1,
            "majority-side node should still see its groupmate"
        );
    }

    #[test]
    fn test_dead_node_decays_from_peer_view() {
        let cfg = SimConfig::default();
        let mut cluster = SimCluster::new(cfg, constant_workload(600.0), 1);
        // Promotion needs one 8s estimator window of over-threshold rate
        for _ in 0..900 {
            cluster.run_tick();
        }
        assert_eq!(hot_limiter(&cluster, 0, "test").num_peers(), 2);

        cluster.apply_event(&SimEvent::NodeDown(2));
        for _ in 0..1200 {
            cluster.run_tick();
        }
        assert_eq!(
            hot_limiter(&cluster, 0, "test").num_peers(),
            1,
            "dead peer should stop counting after stale_timeout"
        );
    }

    #[test]
    fn test_below_threshold_scope_stays_tail() {
        // 30 rps offered against a 300 rps cluster limit: estimated
        // utilization ~10% — far below the promotion threshold, so the
        // scope must stay in the tail tier with no gossip payload
        let cfg = SimConfig::default();
        let mut cluster = SimCluster::new(cfg, constant_workload(30.0), 1);
        for _ in 0..3000 {
            cluster.run_tick();
        }
        for i in 0..3 {
            assert_eq!(
                cluster.scope_tier(i, "test"),
                Some("tail"),
                "low-utilization scope must stay tail on node {}",
                i
            );
            assert_eq!(cluster.hot_scopes(i), 0);
            // Membership liveness still flows without hot scopes
            assert_eq!(cluster.nodes[i].live_peers, 2);
            // Tail visibility: the per-pattern aggregate carries the
            // unpromoted volume (~10 rps/node of the 30 rps offered)
            let tail = cluster.tail_rate(i, cluster.sim_time());
            assert!(
                (tail - 10.0).abs() < 3.0,
                "node {} tail aggregate {:.1}, expected ~10",
                i,
                tail
            );
            // ...and peers see it through gossip
            let peer_tail: f64 = cluster.nodes[i].records.values().map(|r| r.tail_rate).sum();
            assert!(
                (peer_tail - 20.0).abs() < 6.0,
                "node {} sees {:.1} rps of peer tail volume, expected ~20",
                i,
                peer_tail
            );
        }
    }

    #[test]
    fn test_hot_scope_demotes_after_load_drops() {
        let workloads = vec![Workload::new(
            "test",
            LoadPattern::Step {
                before: 600.0,
                after: 20.0,
                at: Duration::from_secs(10),
            },
        )
        .arrival(ArrivalProcess::Deterministic)];
        let cfg = SimConfig::default();
        let mut cluster = SimCluster::new(cfg, workloads, 1);
        // 10s of heavy load: promoted everywhere
        for _ in 0..1000 {
            cluster.run_tick();
        }
        assert_eq!(cluster.scope_tier(0, "test"), Some("hot"));
        // 30s at ~7% utilization: past the demotion hold everywhere
        for _ in 0..3000 {
            cluster.run_tick();
        }
        for i in 0..3 {
            assert_eq!(
                cluster.scope_tier(i, "test"),
                Some("tail"),
                "idle hot scope must demote on node {}",
                i
            );
        }
    }

    #[test]
    fn test_down_node_receives_no_traffic() {
        let cfg = SimConfig::default();
        let mut cluster = SimCluster::new(cfg, constant_workload(600.0), 1);
        cluster.apply_event(&SimEvent::NodeDown(1));
        let mut node1_accepted = 0;
        for _ in 0..200 {
            let counts = cluster.run_tick();
            node1_accepted += counts.per_node_accepted[1];
        }
        assert_eq!(node1_accepted, 0);
    }
}
