//! Scenario definitions and the run loop.
//!
//! A scenario is cluster config + workloads + a timeline of events. The
//! library below encodes the Milestone 4 core scenarios; the benchmark
//! harness and CI tests both run these, so control-loop changes are always
//! judged against the same situations.

use std::time::Duration;

use super::cluster::{GossipModel, SimCluster, SimConfig, SimEvent};
use super::metrics::{summarize, RunResult, Sample};
use super::workload::{
    ArrivalProcess, LoadPattern, PopulationWorkload, Routing, SineComponent, Workload,
};

#[derive(Debug, Clone)]
pub struct Scenario {
    pub name: String,
    pub cfg: SimConfig,
    pub workloads: Vec<Workload>,
    /// Heavy-tailed per-user populations (Milestone 6)
    pub populations: Vec<PopulationWorkload>,
    /// Timeline events, applied when simulated time reaches them (must be
    /// sorted by time)
    pub events: Vec<(Duration, SimEvent)>,
    pub duration: Duration,
    /// Metrics sampling interval
    pub sample_interval: Duration,
}

impl Scenario {
    pub fn new(name: impl Into<String>, cfg: SimConfig, workloads: Vec<Workload>) -> Self {
        Scenario {
            name: name.into(),
            cfg,
            workloads,
            populations: Vec::new(),
            events: Vec::new(),
            duration: Duration::from_secs(60),
            sample_interval: Duration::from_millis(500),
        }
    }

    pub fn population(mut self, population: PopulationWorkload) -> Self {
        self.populations.push(population);
        self
    }

    pub fn event(mut self, at: Duration, event: SimEvent) -> Self {
        self.events.push((at, event));
        self.events.sort_by_key(|(t, _)| *t);
        self
    }

    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Override the arrival process on every workload (tests use
    /// `Deterministic` for tight assertions).
    pub fn arrival(mut self, arrival: ArrivalProcess) -> Self {
        for w in &mut self.workloads {
            w.arrival = arrival;
        }
        self
    }

    /// Run the scenario with the given seed. Same seed + same scenario =
    /// identical `RunResult`, byte for byte in CSV/JSON form.
    pub fn run(&self, seed: u64) -> RunResult {
        let mut cluster = self.build_cluster(seed);

        let tick = self.cfg.tick;
        let total_ticks = (self.duration.as_nanos() / tick.as_nanos()) as u64;
        let sample_ticks = (self.sample_interval.as_nanos() / tick.as_nanos()).max(1) as u64;
        let sample_dt = tick.as_secs_f64() * sample_ticks as f64;

        let mut samples = Vec::with_capacity((total_ticks / sample_ticks) as usize);
        let mut applied_events: Vec<(f64, String)> = Vec::new();
        let mut event_idx = 0;

        let mut acc_offered = 0u64;
        let mut acc_accepted = 0u64;
        let mut acc_per_node = vec![0u64; self.cfg.num_nodes];

        for tick_i in 0..total_ticks {
            let t = cluster.sim_time();
            while event_idx < self.events.len() && t >= self.events[event_idx].0 {
                let (at, event) = &self.events[event_idx];
                cluster.apply_event(event);
                applied_events.push((at.as_secs_f64(), event.label()));
                event_idx += 1;
            }

            let counts = cluster.run_tick();
            acc_offered += counts.offered;
            acc_accepted += counts.accepted;
            for (i, c) in counts.per_node_accepted.iter().enumerate() {
                acc_per_node[i] += c;
            }

            if (tick_i + 1) % sample_ticks == 0 {
                samples.push(Sample {
                    t: cluster.sim_time().as_secs_f64(),
                    offered_rate: acc_offered as f64 / sample_dt,
                    accepted_rate: acc_accepted as f64 / sample_dt,
                    per_node_rate: acc_per_node.iter().map(|&c| c as f64 / sample_dt).collect(),
                });
                acc_offered = 0;
                acc_accepted = 0;
                acc_per_node.iter_mut().for_each(|c| *c = 0);
            }
        }

        let summary = summarize(
            &samples,
            self.cfg.cluster_target,
            &applied_events,
            sample_dt,
        );

        RunResult {
            scenario: self.name.clone(),
            seed,
            target: self.cfg.cluster_target,
            num_nodes: self.cfg.num_nodes,
            samples,
            events: applied_events,
            summary,
        }
    }

    /// Build the cluster this scenario runs (tests that need per-scope /
    /// tier state drive the returned cluster tick by tick themselves).
    pub fn build_cluster(&self, seed: u64) -> SimCluster {
        SimCluster::with_populations(
            self.cfg.clone(),
            self.workloads.clone(),
            self.populations.clone(),
            seed,
        )
    }
}

// ===== Scenario library (Milestone 4.2) =====
//
// All scenarios use the production-default cluster config (target 300,
// 3 nodes unless stated) and Poisson arrivals. Load levels are expressed
// relative to the cluster target.

fn constant(rate: f64) -> Vec<Workload> {
    vec![Workload::new("api", LoadPattern::Constant { rate })]
}

/// Constant load at 50% of target: nothing should be throttled.
pub fn steady_below() -> Scenario {
    let cfg = SimConfig::default();
    let rate = cfg.cluster_target * 0.5;
    Scenario::new("steady_below", cfg, constant(rate))
}

/// Constant load exactly at target.
pub fn steady_at() -> Scenario {
    let cfg = SimConfig::default();
    let rate = cfg.cluster_target;
    Scenario::new("steady_at", cfg, constant(rate))
}

/// Constant load at 2× target: cluster must hold the line at target.
pub fn steady_above() -> Scenario {
    let cfg = SimConfig::default();
    let rate = cfg.cluster_target * 2.0;
    Scenario::new("steady_above", cfg, constant(rate))
}

/// Load doubles at t=30s (1× → 2× target).
pub fn step_change() -> Scenario {
    let cfg = SimConfig::default();
    let target = cfg.cluster_target;
    Scenario::new(
        "step",
        cfg,
        vec![Workload::new(
            "api",
            LoadPattern::Step {
                before: target,
                after: target * 2.0,
                at: Duration::from_secs(30),
            },
        )],
    )
    .duration(Duration::from_secs(90))
}

/// Load ramps 0 → 3× target over the first 60 seconds.
pub fn ramp() -> Scenario {
    let cfg = SimConfig::default();
    let target = cfg.cluster_target;
    Scenario::new(
        "ramp",
        cfg,
        vec![Workload::new(
            "api",
            LoadPattern::Ramp {
                from: 0.0,
                to: target * 3.0,
                start: Duration::ZERO,
                ramp: Duration::from_secs(60),
            },
        )],
    )
    .duration(Duration::from_secs(120))
}

/// Periodic 10× spikes: 1 second at 10× target every 10 seconds, base 0.5×.
pub fn burst() -> Scenario {
    let cfg = SimConfig::default();
    let target = cfg.cluster_target;
    Scenario::new(
        "burst",
        cfg,
        vec![Workload::new(
            "api",
            LoadPattern::Burst {
                base: target * 0.5,
                spike: target * 10.0,
                period: Duration::from_secs(10),
                spike_duration: Duration::from_secs(1),
            },
        )],
    )
}

/// Third node joins at t=30s under 2× target load.
pub fn node_join() -> Scenario {
    let cfg = SimConfig {
        initially_down: vec![2],
        ..SimConfig::default()
    };
    let rate = cfg.cluster_target * 2.0;
    Scenario::new("join", cfg, constant(rate))
        .event(Duration::from_secs(30), SimEvent::NodeUp(2))
        .duration(Duration::from_secs(90))
}

/// One of three nodes crashes at t=30s under 2× target load. The remaining
/// nodes must absorb its share once its gossiped rate decays.
pub fn node_leave() -> Scenario {
    let cfg = SimConfig::default();
    let rate = cfg.cluster_target * 2.0;
    Scenario::new("leave", cfg, constant(rate))
        .event(Duration::from_secs(30), SimEvent::NodeDown(2))
        .duration(Duration::from_secs(90))
}

/// 5-node cluster partitions 2/3 at t=30s and heals at t=70s, under 2×
/// target load. The Milestone 3 staleness decay must bound overshoot during
/// the partition (each side eventually sees only its own group) and the
/// cluster must re-converge after heal.
pub fn partition_heal() -> Scenario {
    let cfg = SimConfig {
        num_nodes: 5,
        ..SimConfig::default()
    };
    let rate = cfg.cluster_target * 2.0;
    Scenario::new("partition", cfg, constant(rate))
        .event(
            Duration::from_secs(30),
            SimEvent::Partition(vec![vec![0, 1], vec![2, 3, 4]]),
        )
        .event(Duration::from_secs(70), SimEvent::Heal)
        .duration(Duration::from_secs(120))
}

/// One node receives 90% of the traffic (2× target total).
pub fn skewed_load() -> Scenario {
    let cfg = SimConfig::default();
    let rate = cfg.cluster_target * 2.0;
    let workload =
        Workload::new("api", LoadPattern::Constant { rate }).node_weights(vec![0.9, 0.05, 0.05]);
    Scenario::new("skew", cfg, vec![workload]).duration(Duration::from_secs(90))
}

/// Smoothly varying load around the target — the tuning pattern ported from
/// the retired `request_simulator_plot` GUI example (base ± two sine
/// components), scaled to the cluster target.
pub fn sinusoidal() -> Scenario {
    let cfg = SimConfig::default();
    let target = cfg.cluster_target;
    Scenario::new(
        "sinusoidal",
        cfg,
        vec![Workload::new(
            "api",
            LoadPattern::Sinusoidal {
                base: target,
                components: vec![
                    // Same shape as the GUI example's defaults
                    // (amplitude 40% / 20% of base, slow + fast period)
                    SineComponent {
                        amplitude: target * 0.4,
                        frequency_hz: 0.02,
                    },
                    SineComponent {
                        amplitude: target * 0.2,
                        frequency_hz: 0.1,
                    },
                ],
            },
        )],
    )
    .duration(Duration::from_secs(120))
}

/// Rapid scale-up: 3 → 30 nodes, one join per second starting at t=30s,
/// under 2× target load. Exercises repeated setpoint changes and the
/// cold-start admission burst (each fresh node starts with a full
/// cluster-target-sized token bucket — see the cold-start roadmap item).
pub fn autoscale() -> Scenario {
    let cfg = SimConfig {
        num_nodes: 30,
        initially_down: (3..30).collect(),
        ..SimConfig::default()
    };
    let rate = cfg.cluster_target * 2.0;
    let mut s = Scenario::new("autoscale", cfg, constant(rate)).duration(Duration::from_secs(120));
    for (k, node) in (3..30).enumerate() {
        s = s.event(Duration::from_secs(30 + k as u64), SimEvent::NodeUp(node));
    }
    s
}

/// Broad outage: half of a 10-node cluster crashes simultaneously at t=30s
/// under 2× target load. Staleness decay runs per-peer in parallel, so
/// recovery should take the same stale_timeout + settle as a single leave.
pub fn mass_outage() -> Scenario {
    let cfg = SimConfig {
        num_nodes: 10,
        ..SimConfig::default()
    };
    let rate = cfg.cluster_target * 2.0;
    let mut s =
        Scenario::new("mass_outage", cfg, constant(rate)).duration(Duration::from_secs(120));
    for node in 5..10 {
        s = s.event(Duration::from_secs(30), SimEvent::NodeDown(node));
    }
    s
}

/// Degraded network: 30% of gossip messages lost for the whole run, 2×
/// target load. The current PID uses only the local rate as its feedback
/// signal, so loss delays membership awareness but cannot destabilize the
/// loop; Milestone 5 estimation engines consume gossip observations
/// directly and are expected to be more sensitive — this is their noise-
/// robustness baseline.
pub fn lossy() -> Scenario {
    let cfg = SimConfig {
        gossip: GossipModel {
            loss: 0.3,
            ..GossipModel::default()
        },
        ..SimConfig::default()
    };
    let rate = cfg.cluster_target * 2.0;
    Scenario::new("lossy", cfg, constant(rate)).duration(Duration::from_secs(90))
}

/// High gossip lag: 2s ± 1s propagation delay (4 sync intervals), 2×
/// target load. Validates the documented claim that the conservative gains
/// tolerate multi-second gossip lag.
pub fn laggy() -> Scenario {
    let cfg = SimConfig {
        gossip: GossipModel {
            delay: Duration::from_secs(2),
            jitter: Duration::from_secs(1),
            ..GossipModel::default()
        },
        ..SimConfig::default()
    };
    let rate = cfg.cluster_target * 2.0;
    Scenario::new("laggy", cfg, constant(rate)).duration(Duration::from_secs(90))
}

/// Heavy gossip loss: 60% of messages dropped for the whole run, 2× target
/// load. Milestone 5 noise-robustness variant: engines that consume gossip
/// observations directly (bayesian, hybrid) see a sparse, gappy sample
/// stream — their estimates must widen, not destabilize.
pub fn lossy_heavy() -> Scenario {
    let cfg = SimConfig {
        gossip: GossipModel {
            loss: 0.6,
            ..GossipModel::default()
        },
        ..SimConfig::default()
    };
    let rate = cfg.cluster_target * 2.0;
    Scenario::new("lossy_heavy", cfg, constant(rate)).duration(Duration::from_secs(90))
}

/// High jitter: 100ms base delay with up to 3s of per-message jitter, 2×
/// target load. Milestone 5 noise-robustness variant: observations arrive
/// out of order relative to their generation times, exercising the
/// estimators' new-sample detection.
pub fn jittery() -> Scenario {
    let cfg = SimConfig {
        gossip: GossipModel {
            delay: Duration::from_millis(100),
            jitter: Duration::from_secs(3),
            ..GossipModel::default()
        },
        ..SimConfig::default()
    };
    let rate = cfg.cluster_target * 2.0;
    Scenario::new("jittery", cfg, constant(rate)).duration(Duration::from_secs(90))
}

/// Congestion coupling: user traffic saturates the links, so gossip blacks
/// out completely from t=30s to t=60s while 2× target load continues. Once
/// records decay past stale_timeout each node sees zero live peers and
/// re-targets the full cluster target — the soft-limit worst case, here
/// triggered *by* the overload itself. The link drains at t=60s and the
/// cluster must re-converge.
pub fn congestion() -> Scenario {
    let cfg = SimConfig::default();
    let rate = cfg.cluster_target * 2.0;
    Scenario::new("congestion", cfg, constant(rate))
        .event(Duration::from_secs(30), SimEvent::GossipLoss(1.0))
        .event(Duration::from_secs(60), SimEvent::GossipLoss(0.0))
        .duration(Duration::from_secs(120))
}

/// Steady 2× target load on an n-node cluster (scale sweep).
pub fn scale(num_nodes: usize) -> Scenario {
    let cfg = SimConfig {
        num_nodes,
        ..SimConfig::default()
    };
    let rate = cfg.cluster_target * 2.0;
    Scenario::new(format!("scale_{}", num_nodes), cfg, constant(rate))
}

// ===== Milestone 6 two-tier scenarios =====
//
// Per-user limiting: the cluster target is the *per-user* limit (each
// scope gets the full pattern target), so these use a small target (10
// rps/user) and a heavy-tailed population. Only the head of the Zipf
// distribution should ever gossip.

/// Zipf(1.0) population of 100k users, uniform load-balancer routing,
/// 60× the per-user limit of total volume: the head few users must be
/// capped at the limit via promotion; the tail must stay local.
pub fn pareto_users() -> Scenario {
    let cfg = SimConfig::default().with_cluster_target(10.0);
    let total = 600.0; // top-rank user offered ≈ 50 rps ≫ the 10 rps limit
    Scenario::new("pareto_users", cfg, Vec::new()).population(PopulationWorkload::new(
        "user:",
        100_000,
        1.0,
        LoadPattern::Constant { rate: total },
    ))
}

/// Session-affinity variant of `pareto_users`: every user's traffic lands
/// on one node, the worst case for the `local_rate × num_nodes` promotion
/// estimate. Skew makes the hot node promote *earlier* (its local rate
/// overestimates the cluster rate), so the failure mode is under-service
/// of mid-band users, not overage — quantified in tests/simulation.rs.
pub fn sticky_users() -> Scenario {
    let cfg = SimConfig::default().with_cluster_target(10.0);
    Scenario::new("sticky_users", cfg, Vec::new()).population(
        PopulationWorkload::new("user:", 100_000, 1.0, LoadPattern::Constant { rate: 600.0 })
            .routing(Routing::Sticky),
    )
}

/// One user's journey tail → hot → tail on top of a light background
/// population: 2 rps (tail) for 30s, 30 rps (3× the 10 rps limit) until
/// t=60s, then back to 2 rps. Exercises promotion lag, coordinated capping
/// at the limit, and demotion hysteresis.
pub fn user_ramp() -> Scenario {
    let cfg = SimConfig::default().with_cluster_target(10.0);
    Scenario::new(
        "user_ramp",
        cfg,
        vec![Workload::new(
            "user:ramp",
            LoadPattern::Piecewise {
                steps: vec![
                    (Duration::ZERO, 2.0),
                    (Duration::from_secs(30), 30.0),
                    (Duration::from_secs(60), 2.0),
                ],
            },
        )],
    )
    .population(PopulationWorkload::new(
        "user:",
        10_000,
        1.0,
        LoadPattern::Constant { rate: 60.0 },
    ))
    .duration(Duration::from_secs(120))
}

/// All library scenarios (scale sweep at 2, 5, 10, 50 nodes).
pub fn library() -> Vec<Scenario> {
    vec![
        steady_below(),
        steady_at(),
        steady_above(),
        step_change(),
        ramp(),
        burst(),
        node_join(),
        node_leave(),
        partition_heal(),
        skewed_load(),
        sinusoidal(),
        autoscale(),
        mass_outage(),
        lossy(),
        lossy_heavy(),
        jittery(),
        laggy(),
        congestion(),
        pareto_users(),
        sticky_users(),
        user_ramp(),
        scale(2),
        scale(5),
        scale(10),
        scale(50),
    ]
}

/// Look up a library scenario by name.
pub fn by_name(name: &str) -> Option<Scenario> {
    library().into_iter().find(|s| s.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_library_names_unique() {
        let lib = library();
        let mut names: Vec<&str> = lib.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), lib.len());
    }

    #[test]
    fn test_by_name() {
        assert!(by_name("partition").is_some());
        assert!(by_name("no_such_scenario").is_none());
    }

    #[test]
    fn test_short_run_produces_samples() {
        let result = steady_at().duration(Duration::from_secs(5)).run(1);
        assert_eq!(result.samples.len(), 10); // 5s / 500ms
        assert_eq!(result.num_nodes, 3);
    }
}
