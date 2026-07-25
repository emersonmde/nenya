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
use crate::pid_controller::PIDControllerBuilder;
use crate::{RateLimiter, RateLimiterBuilder};

pub use crate::engine::BayesianParams;

use super::rng::SplitMix64;
use super::workload::{ArrivalProcess, Workload};

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
            estimator: None,
            tick: Duration::from_millis(10),
            gossip: GossipModel::default(),
            initially_down: Vec::new(),
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
    /// Simulated time the record last changed (age-at-receipt, mirroring
    /// `GossipManager`'s local-monotonic-clock bookkeeping)
    received_at: Duration,
}

struct SimNode {
    up: bool,
    limiters: BTreeMap<String, RateLimiter<f64>>,
    records: BTreeMap<usize, PeerRecord>,
    /// Fractional-arrival accumulator per scope (deterministic arrivals)
    accum: BTreeMap<String, f64>,
    rng: SplitMix64,
}

struct Delivery {
    to: usize,
    from: usize,
    rates: HashMap<String, f64>,
}

/// Per-tick request counters.
#[derive(Debug, Clone)]
pub struct TickCounts {
    pub offered: u64,
    pub accepted: u64,
    pub per_node_accepted: Vec<u64>,
}

pub struct SimCluster {
    cfg: SimConfig,
    workloads: Vec<Workload>,
    nodes: Vec<SimNode>,
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
        assert!(cfg.num_nodes > 0, "cluster needs at least one node");
        assert!(!cfg.tick.is_zero(), "tick must be positive");

        let mut root_rng = SplitMix64::new(seed);
        let bus_rng = root_rng.fork();

        let start = Instant::now();
        let sync_ticks = (cfg.sync_interval.as_nanos() / cfg.tick.as_nanos()).max(1) as u64;

        let mut nodes = Vec::with_capacity(cfg.num_nodes);
        for i in 0..cfg.num_nodes {
            let up = !cfg.initially_down.contains(&i);
            let mut node = SimNode {
                up,
                limiters: BTreeMap::new(),
                records: BTreeMap::new(),
                accum: BTreeMap::new(),
                rng: root_rng.fork(),
            };
            if up {
                for w in &workloads {
                    node.limiters
                        .insert(w.scope.clone(), make_limiter(&cfg, start));
                }
            }
            nodes.push(node);
        }

        // Spread publish ticks evenly across the sync interval
        let phase: Vec<u64> = (0..cfg.num_nodes as u64)
            .map(|i| i * sync_ticks / cfg.num_nodes as u64)
            .collect();

        SimCluster {
            group: vec![0; cfg.num_nodes],
            nodes,
            workloads,
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

    /// Total offered rate across workloads at simulated time `t`.
    pub fn offered_rate_at(&self, t: Duration) -> f64 {
        self.workloads.iter().map(|w| w.pattern.rate_at(t)).sum()
    }

    pub fn node_is_up(&self, i: usize) -> bool {
        self.nodes[i].up
    }

    /// Apply a timeline event at the current simulated time.
    pub fn apply_event(&mut self, event: &SimEvent) {
        match event {
            SimEvent::NodeDown(i) => {
                let node = &mut self.nodes[*i];
                node.up = false;
                // Crash loses all state
                node.limiters.clear();
                node.records.clear();
                node.accum.clear();
            }
            SimEvent::NodeUp(i) => {
                let now = self.now();
                let node = &mut self.nodes[*i];
                node.up = true;
                node.limiters.clear();
                node.records.clear();
                node.accum.clear();
                for w in &self.workloads {
                    node.limiters
                        .insert(w.scope.clone(), make_limiter(&self.cfg, now));
                }
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
                let limiter = node
                    .limiters
                    .get_mut(&scope)
                    .expect("limiter exists for every workload scope");
                for _ in 0..n_arrivals {
                    counts.offered += 1;
                    if !limiter.should_throttle_at(now) {
                        counts.accepted += 1;
                        counts.per_node_accepted[i] += 1;
                    }
                }
            }
        }

        self.tick_index += 1;
        counts
    }

    /// One node's sync-loop pass, mirroring `gossip_sync_loop` step for step.
    fn sync_node(&mut self, i: usize, now: Instant, t: Duration) {
        // Step 1: refresh and collect local per-scope rates
        let mut local_rates = HashMap::new();
        {
            let node = &mut self.nodes[i];
            for (scope, limiter) in node.limiters.iter_mut() {
                limiter.update_state_at(now);
                local_rates.insert(scope.clone(), limiter.local_accepted_request_rate());
            }
        }

        // Step 2: publish to every reachable peer through the message bus
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
                });
        }

        // Step 3: aggregate peer observations with the production decay code
        let observations: Vec<PeerObservation> = self.nodes[i]
            .records
            .iter()
            .map(|(peer, rec)| PeerObservation {
                node_id: peer.to_string(),
                age: t.saturating_sub(rec.received_at),
                scope_rates: rec.rates.clone(),
            })
            .collect();
        let aggregated = aggregate_peer_rates(
            &observations,
            self.cfg.sync_interval,
            self.cfg.stale_timeout,
        );

        // Step 4: apply external rates, live peer count, and per-peer
        // observations in one pass; scopes absent from the aggregate are
        // explicitly zeroed. The aggregated values remain the transport's
        // reporting metrics; the engine consumes the raw observations.
        let node = &mut self.nodes[i];
        for (scope, limiter) in node.limiters.iter_mut() {
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
        }
    }
}

fn make_limiter(cfg: &SimConfig, now: Instant) -> RateLimiter<f64> {
    // Mirrors RateLimitManager::create_limiter_from_pattern for a
    // distributed-mode scope: target == cluster target, PID seeded with the
    // same gains, default bucket capacity and update interval
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

    let mut builder = RateLimiterBuilder::new(cfg.cluster_target)
        .cluster_target(cfg.cluster_target)
        .min_rate(cfg.min_rate)
        .max_rate(cfg.max_rate)
        .update_interval(cfg.pid_update_interval)
        .initial_timestamp(now);
    if let Some(k) = cfg.min_window_samples {
        builder = builder.min_window_samples(k);
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

    #[test]
    fn test_gossip_reaches_peers() {
        let cfg = SimConfig::default();
        let mut cluster = SimCluster::new(cfg, constant_workload(600.0), 1);
        // Run 5 simulated seconds: plenty for publish + delivery
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
            let limiter = cluster.nodes[i].limiters.get("test").unwrap();
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
            cluster.nodes[0].limiters.get("test").unwrap().num_peers(),
            0,
            "isolated node should see no live peers after stale_timeout"
        );
        assert_eq!(
            cluster.nodes[1].limiters.get("test").unwrap().num_peers(),
            1,
            "majority-side node should still see its groupmate"
        );
    }

    #[test]
    fn test_dead_node_decays_from_peer_view() {
        let cfg = SimConfig::default();
        let mut cluster = SimCluster::new(cfg, constant_workload(600.0), 1);
        for _ in 0..300 {
            cluster.run_tick();
        }
        assert_eq!(
            cluster.nodes[0].limiters.get("test").unwrap().num_peers(),
            2
        );

        cluster.apply_event(&SimEvent::NodeDown(2));
        for _ in 0..1200 {
            cluster.run_tick();
        }
        assert_eq!(
            cluster.nodes[0].limiters.get("test").unwrap().num_peers(),
            1,
            "dead peer should stop counting after stale_timeout"
        );
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
