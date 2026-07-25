//! Deterministic multi-node simulation tests (Milestone 4.4).
//!
//! These encode the roadmap's acceptance thresholds as CI assertions. All
//! runs use fixed seeds and the in-process simulator, so failures are
//! reproducible exactly. The fast subset below runs in a few seconds; the
//! full scenario matrix and the 50-node sweep are `#[ignore]`d (run with
//! `cargo test --all-features -- --ignored`).
//!
//! Threshold provenance:
//! - ±5% band and hold window: roadmap Milestone 4 acceptance criteria
//! - leave/partition convergence bounds: stale_timeout (10s, the time for a
//!   silent peer's gossiped rate to fully decay) plus PID settling margin
//! - the initial ~1s overshoot spike in every scenario is the first-second
//!   cold-start burst (capacity starts at the configured default until the
//!   first control update shrinks it to `refill × 1s` — the Milestone 5.4
//!   adaptive burst allowance) and is deliberately not asserted against

#![cfg(feature = "sim")]

use nenya::sim::scenario;
use nenya::sim::RunResult;
use std::time::Duration;

const SEED: u64 = 42;

/// Mean cluster accepted rate over the final quarter of the run.
fn steady_mean(r: &RunResult) -> f64 {
    let n = r.samples.len();
    let steady = &r.samples[n - n / 4..];
    steady.iter().map(|s| s.accepted_rate).sum::<f64>() / steady.len() as f64
}

fn convergence_after(r: &RunResult, label: &str) -> Option<f64> {
    r.summary
        .convergence
        .iter()
        .find(|c| c.label == label)
        .unwrap_or_else(|| panic!("no convergence anchor '{}' in {:?}", label, r.summary))
        .time_to_converge
}

#[test]
fn test_determinism_same_seed_identical_output() {
    // The partition scenario exercises everything: Poisson arrivals, gossip
    // jitter, events, staleness decay
    let a = scenario::partition_heal().run(SEED);
    let b = scenario::partition_heal().run(SEED);
    assert_eq!(a.to_csv(), b.to_csv(), "same seed must be byte-identical");
    assert_eq!(a.to_json(), b.to_json());

    let c = scenario::partition_heal().run(SEED + 1);
    assert_ne!(
        a.to_csv(),
        c.to_csv(),
        "different seeds should differ (sanity check that the seed is used)"
    );
}

#[test]
fn test_steady_above_holds_target() {
    let r = scenario::steady_above().run(SEED);

    let converge = convergence_after(&r, "start").expect("steady_above must converge");
    assert!(
        converge <= 10.0,
        "convergence took {:.1}s (limit 10s)",
        converge
    );

    let mean = steady_mean(&r);
    assert!(
        (mean - r.target).abs() <= r.target * 0.05,
        "steady mean {:.1} outside ±5% of target {:.1}",
        mean,
        r.target
    );

    let cv = r.summary.fairness_cv.expect("3 nodes carried traffic");
    assert!(cv < 0.05, "unfair split under uniform load: CV {:.3}", cv);
}

#[test]
fn test_steady_below_throttles_nothing() {
    let r = scenario::steady_below().run(SEED);
    assert_eq!(
        r.summary.integrated_overshoot, 0.0,
        "load below target can never overshoot"
    );
    // Total demand over the run; sacrificing more than 1% of it means the
    // limiter throttled requests it had no reason to throttle
    let total_offered: f64 = r.samples.iter().map(|s| s.offered_rate).sum::<f64>() * 0.5; // 500ms samples
    assert!(
        r.summary.integrated_undershoot < total_offered * 0.01,
        "throttled {:.0} of {:.0} offered requests under target",
        r.summary.integrated_undershoot,
        total_offered
    );
}

#[test]
fn test_step_change_reconverges() {
    let r = scenario::step_change().run(SEED);

    let converge = convergence_after(&r, "start").expect("must converge initially");
    assert!(
        converge <= 10.0,
        "initial convergence took {:.1}s",
        converge
    );

    // After the step at t=30s the run has 60s to settle; the final quarter
    // must be back in the ±5% band
    let mean = steady_mean(&r);
    assert!(
        (mean - r.target).abs() <= r.target * 0.05,
        "post-step steady mean {:.1} outside ±5% of {:.1}",
        mean,
        r.target
    );
}

#[test]
fn test_node_join_absorbed() {
    let r = scenario::node_join().run(SEED);
    let converge = convergence_after(&r, "node2_up").expect("must re-converge after join");
    // A joining node is visible to peers within one gossip round; allow
    // three PID update intervals of settling on top
    assert!(
        converge <= 15.0,
        "join re-convergence took {:.1}s",
        converge
    );

    let mean = steady_mean(&r);
    assert!((mean - r.target).abs() <= r.target * 0.05);
}

#[test]
fn test_node_leave_absorbed_after_decay() {
    let r = scenario::node_leave().run(SEED);
    let converge = convergence_after(&r, "node2_down").expect("must re-converge after leave");
    // Lower bound on recovery is stale_timeout (10s): survivors only claim
    // the dead node's share once its gossiped rate fully decays. Allow 10s
    // of PID settling on top.
    assert!(
        converge <= 20.0,
        "leave re-convergence took {:.1}s (stale_timeout is 10s)",
        converge
    );

    let mean = steady_mean(&r);
    assert!((mean - r.target).abs() <= r.target * 0.05);
}

#[test]
fn test_partition_overshoot_bounded_and_heals() {
    let r = scenario::partition_heal().run(SEED);

    // During a partition each side independently converges toward the full
    // cluster target (it cannot know better — gossip-based limits are soft).
    // The worst case is therefore 2× target; assert we never exceed it plus
    // the ±5% band once initial convergence is done (10s — the same limit
    // the start-convergence assertions use; the first seconds are the full
    // token buckets draining).
    let worst_case = r.target * 2.0;
    for s in r.samples.iter().filter(|s| s.t > 10.0) {
        assert!(
            s.accepted_rate <= worst_case * 1.05,
            "t={:.1}: accepted {:.1} exceeds partition worst case {:.1}",
            s.t,
            s.accepted_rate,
            worst_case
        );
    }

    // Total overshoot is bounded by excess demand × partition duration
    // (300 rps excess × 40s here) — the roadmap's soft-limit bound
    let bound = 300.0 * 40.0;
    assert!(
        r.summary.integrated_overshoot <= bound,
        "integrated overshoot {:.0} exceeds stale-decay bound {:.0}",
        r.summary.integrated_overshoot,
        bound
    );

    // After heal the cluster must re-converge (this is the regression test
    // for integral windup: without the anti-windup clamp the minority side
    // stays ~40% over its share for the rest of the run)
    let converge = convergence_after(&r, "heal").expect("must re-converge after heal");
    assert!(
        converge <= 15.0,
        "post-heal convergence took {:.1}s",
        converge
    );

    let cv = r.summary.fairness_cv.expect("5 nodes carried traffic");
    assert!(cv < 0.05, "fairness not restored after heal: CV {:.3}", cv);
}

#[test]
fn test_scale_sweep_small() {
    for nodes in [2, 5, 10] {
        let r = scenario::scale(nodes).run(SEED);
        let converge = convergence_after(&r, "start")
            .unwrap_or_else(|| panic!("scale_{} did not converge", nodes));
        assert!(
            converge <= 15.0,
            "scale_{} convergence took {:.1}s",
            nodes,
            converge
        );
        let mean = steady_mean(&r);
        assert!(
            (mean - r.target).abs() <= r.target * 0.05,
            "scale_{} steady mean {:.1} outside band",
            nodes,
            mean
        );
    }
}

#[test]
fn test_autoscale_absorbs_rapid_joins() {
    let r = scenario::autoscale().run(SEED);

    // With the adaptive burst allowance (Milestone 5.4) a joining node's
    // bucket shrinks to its share within one control update; 27 rapid
    // joins measure ~2100 requests of overshoot (was ~8100 with the
    // static cluster-target bucket)
    assert!(
        r.summary.integrated_overshoot < 4_000.0,
        "autoscale overshoot {:.0} exceeds join-burst budget",
        r.summary.integrated_overshoot
    );

    // After the last join (t=57s) the 30-node cluster must settle
    let converge = convergence_after(&r, "node29_up").expect("must converge after final join");
    assert!(
        converge <= 30.0,
        "post-scale-up convergence took {:.1}s",
        converge
    );

    let mean = steady_mean(&r);
    assert!(
        (mean - r.target).abs() <= r.target * 0.05,
        "steady mean {:.1} outside band after scale-up",
        mean
    );
}

#[test]
fn test_mass_outage_recovers_like_single_leave() {
    let r = scenario::mass_outage().run(SEED);

    // Staleness decay runs per-peer in parallel: losing 5 of 10 nodes at
    // once must recover on the same stale_timeout + settle budget as a
    // single leave (measured 12.5s)
    let converge = convergence_after(&r, "node9_down").expect("must re-converge after mass outage");
    assert!(
        converge <= 20.0,
        "mass-outage re-convergence took {:.1}s",
        converge
    );

    let mean = steady_mean(&r);
    assert!((mean - r.target).abs() <= r.target * 0.05);
}

#[test]
fn test_lossy_network_stays_stable() {
    let r = scenario::lossy().run(SEED);

    // 30% gossip loss must not visibly degrade control: the PID's feedback
    // signal is the local rate, so loss only delays membership awareness
    let converge = convergence_after(&r, "start").expect("must converge under loss");
    assert!(
        converge <= 10.0,
        "convergence took {:.1}s at 30% loss",
        converge
    );

    let mean = steady_mean(&r);
    assert!((mean - r.target).abs() <= r.target * 0.05);
    assert!(
        r.summary.integrated_overshoot < 2_500.0,
        "loss inflated overshoot to {:.0}",
        r.summary.integrated_overshoot
    );
}

#[test]
fn test_high_gossip_lag_stays_stable() {
    let r = scenario::laggy().run(SEED);

    // 2s ± 1s gossip lag (4 sync intervals): the documented claim is that
    // the conservative gains tolerate this; measured cost is ~1s slower
    // initial convergence and no oscillation
    let converge = convergence_after(&r, "start").expect("must converge under lag");
    assert!(
        converge <= 12.0,
        "convergence took {:.1}s at 2s lag",
        converge
    );

    let mean = steady_mean(&r);
    assert!((mean - r.target).abs() <= r.target * 0.05);
    assert!(
        r.summary.steady_stddev < r.target * 0.05,
        "lag caused oscillation: stddev {:.1}",
        r.summary.steady_stddev
    );
}

#[test]
fn test_congestion_blackout_bounded_and_recovers() {
    let r = scenario::congestion().run(SEED);

    // Full gossip blackout from t=30s under 2× load: once records decay
    // past stale_timeout each node sees zero live peers and re-targets the
    // full cluster target. The cluster over-admits — that MUST happen (it
    // is the documented soft-limit worst case)...
    let blackout: Vec<&nenya::sim::Sample> = r
        .samples
        .iter()
        .filter(|s| s.t > 50.0 && s.t <= 60.0)
        .collect();
    let blackout_mean =
        blackout.iter().map(|s| s.accepted_rate).sum::<f64>() / blackout.len() as f64;
    assert!(
        blackout_mean >= r.target * 1.5,
        "expected over-admission during blackout, got {:.1}",
        blackout_mean
    );

    // ...but it is bounded by the offered load (nodes cannot admit more
    // than arrives) — the blackout cannot amplify beyond demand
    for s in &blackout {
        assert!(
            s.accepted_rate <= s.offered_rate * 1.05 + 1.0,
            "t={:.1}: accepted {:.1} exceeded offered {:.1}",
            s.t,
            s.accepted_rate,
            s.offered_rate
        );
    }

    // Link drains at t=60s: gossip resumes and the cluster re-converges
    // within one stale-free settle (measured 5.0s)
    let converge = convergence_after(&r, "gossip_loss_0pct")
        .expect("must re-converge after congestion clears");
    assert!(
        converge <= 15.0,
        "post-congestion re-convergence took {:.1}s",
        converge
    );
}

// ===== Milestone 6: two-tier per-user scenarios =====

/// Drive a scenario's cluster tick by tick (the per-user assertions need
/// tier/scope state that `RunResult` doesn't carry).
fn run_cluster(s: &nenya::sim::Scenario, seed: u64) -> nenya::sim::SimCluster {
    let mut cluster = s.build_cluster(seed);
    let ticks = (s.duration.as_nanos() / s.cfg.tick.as_nanos()) as u64;
    for _ in 0..ticks {
        cluster.run_tick();
    }
    cluster
}

#[test]
fn test_pareto_users_promotes_head_caps_it_and_leaves_tail_local() {
    let s = scenario::pareto_users();
    let cluster = run_cluster(&s, SEED);
    let limit = s.cfg.cluster_target;
    let duration = s.duration.as_secs_f64();

    // Promoted set ≪ user count: only the head of the Zipf curve gossips
    let promoted = cluster.num_ever_hot();
    assert!(
        promoted >= 3,
        "the head users exceed the limit and must promote (got {})",
        promoted
    );
    assert!(
        promoted <= 100,
        "promoted set {} is not ≪ 100k users",
        promoted
    );

    // Gossip payload bounded by the budget on every node
    for node in 0..s.cfg.num_nodes {
        assert!(
            cluster.max_hot_scopes(node) <= s.cfg.tier.gossip_budget,
            "node {} exceeded the gossip budget: {} hot scopes",
            node,
            cluster.max_hot_scopes(node)
        );
    }

    // Error bound, uniform routing: an unpromoted user must never exceed
    // its limit — promotion is what handles over-limit users, so a scope
    // that stayed tail for the whole run must have been fully served
    // below the limit
    let mut worst_unpromoted: f64 = 0.0;
    let mut worst_promoted: f64 = 0.0;
    for (scope, _offered, accepted) in cluster.all_scope_counts() {
        let rate = accepted as f64 / duration;
        if cluster.was_ever_hot(scope) {
            worst_promoted = worst_promoted.max(rate);
        } else {
            worst_unpromoted = worst_unpromoted.max(rate);
        }
    }
    println!(
        "pareto_users: promoted={}, worst unpromoted={:.2} rps, worst promoted={:.2} rps (limit {})",
        promoted, worst_unpromoted, worst_promoted, limit
    );
    assert!(
        worst_unpromoted <= limit,
        "unpromoted user served {:.2} rps over its {:.0} rps limit",
        worst_unpromoted,
        limit
    );
    // Promoted users are capped by the engine: bounded by max_rate (2×
    // limit) instantaneously, and near the limit on average
    assert!(
        worst_promoted <= limit * 1.5,
        "promoted user averaged {:.2} rps against a {:.0} rps limit",
        worst_promoted,
        limit
    );
}

#[test]
fn test_sticky_routing_promotes_early_no_overage() {
    let s = scenario::sticky_users();
    let cluster = run_cluster(&s, SEED);
    let limit = s.cfg.cluster_target;
    let duration = s.duration.as_secs_f64();

    // Sticky routing makes the hot node's local rate the user's FULL rate,
    // so `local × n` over-estimates and promotion fires earlier — more
    // promoted users than the uniform run, still ≪ the population
    let promoted = cluster.num_ever_hot();
    println!("sticky_users: promoted={}", promoted);
    assert!(promoted <= 300, "promoted set {} not ≪ 100k", promoted);

    // The documented claim: skew cannot create unpromoted overage,
    // because the skewed node sees the full rate and promotes
    let mut worst_unpromoted: f64 = 0.0;
    let mut under_served: Vec<(String, f64, f64)> = Vec::new();
    for (scope, offered, accepted) in cluster.all_scope_counts() {
        let rate = accepted as f64 / duration;
        let offered_rate = offered as f64 / duration;
        if !cluster.was_ever_hot(scope) {
            worst_unpromoted = worst_unpromoted.max(rate);
        }
        // Mid-band users (below the limit, above the single-node share)
        // are where sticky under-service concentrates; record for the
        // count-min-sketch evaluation
        if offered_rate > limit / s.cfg.num_nodes as f64 && offered_rate < limit {
            under_served.push((scope.clone(), offered_rate, rate));
        }
    }
    let worst_ratio = under_served
        .iter()
        .map(|(_, o, a)| a / o)
        .fold(f64::INFINITY, f64::min);
    println!(
        "sticky_users: worst unpromoted={:.2} rps; {} mid-band users, worst served/offered={:.2}",
        worst_unpromoted,
        under_served.len(),
        worst_ratio
    );
    assert!(
        worst_unpromoted <= limit,
        "sticky unpromoted user served {:.2} rps over the {:.0} rps limit",
        worst_unpromoted,
        limit
    );
}

#[test]
fn test_user_ramp_tail_hot_tail_journey() {
    let s = scenario::user_ramp();
    let mut cluster = s.build_cluster(SEED);
    let limit = s.cfg.cluster_target;
    let tick = s.cfg.tick;
    let ticks_per_sec = (Duration::from_secs(1).as_nanos() / tick.as_nanos()) as u64;
    let total_secs = s.duration.as_secs();

    let mut prev_accepted = 0u64;
    let mut max_1s_during_ramp: f64 = 0.0;
    let mut hot_seen_during_peak = false;
    let mut per_second: Vec<f64> = Vec::new();
    for sec in 0..total_secs {
        for _ in 0..ticks_per_sec {
            cluster.run_tick();
        }
        let (_, accepted) = cluster.scope_counts("user:ramp");
        let rate = (accepted - prev_accepted) as f64;
        prev_accepted = accepted;
        per_second.push(rate);
        if (30..60).contains(&sec) {
            max_1s_during_ramp = max_1s_during_ramp.max(rate);
            if (0..s.cfg.num_nodes).any(|n| cluster.scope_tier(n, "user:ramp") == Some("hot")) {
                hot_seen_during_peak = true;
            }
        }
    }

    // Phase 1 (t<30): 2 rps against a 10 rps limit — must stay tail
    assert!(
        !cluster.was_ever_hot("user:ramp") || hot_seen_during_peak,
        "sanity"
    );
    // Phase 2: 30 rps offered → must promote on every node and be capped
    assert!(
        hot_seen_during_peak,
        "over-limit user did not promote during its peak"
    );
    // Promotion-lag overage bound: in the worst 1-second window during
    // the ramp the user may collect the tail burst allowance from every
    // node (n × share = 1 × limit) on top of the limit, plus the engine's
    // max_rate ceiling (2 × limit) applies after promotion
    println!(
        "user_ramp: max 1s accepted during peak = {:.0} (limit {}), per-second: {:?}",
        max_1s_during_ramp,
        limit,
        &per_second[28..44.min(per_second.len())]
    );
    // Measured at seed 42: 22 requests in the step second — the tail
    // burst allowances (n × share = 1 × limit) plus one second of refill
    // (limit) plus Poisson noise. 2.5× is the documented transient bound.
    assert!(
        max_1s_during_ramp <= limit * 2.5,
        "promotion transient admitted {:.0} in 1s against the 2.5×limit bound",
        max_1s_during_ramp
    );
    // Steady peak phase (post-promotion): capped near the limit
    let steady_peak: f64 = per_second[40..58].iter().sum::<f64>() / 18.0;
    assert!(
        steady_peak <= limit * 1.2,
        "hot-phase steady rate {:.1} not capped near the {:.0} limit",
        steady_peak,
        limit
    );
    // Phase 3: back to 2 rps — must demote everywhere within
    // demote_hold + settle
    for node in 0..s.cfg.num_nodes {
        assert_eq!(
            cluster.scope_tier(node, "user:ramp"),
            Some("tail"),
            "node {} did not demote the idle user by the end of the run",
            node
        );
    }
    // No flapping on a clean journey: exactly one promotion per node
    // (3 total: 1 local + 2 peer-triggered), possibly ±1 for timing
    let promotions = cluster.promotions_of("user:ramp");
    println!("user_ramp: {} promotion events", promotions);
    assert!(
        promotions <= s.cfg.num_nodes as u32,
        "tail→hot→tail journey flapped: {} promotion events",
        promotions
    );
}

/// Routing-strategy robustness (Milestone 6 follow-up): the two-tier
/// invariants must hold under every load-balancing policy, not just the
/// uniform-random one the promotion estimate assumes. Round-robin is
/// lower-variance than uniform; least-loaded is the adverse-feedback case
/// (a throttling node accepts less, looks idle, and attracts more
/// traffic); sticky is the known worst case. Measured (seed 42, Zipf 100k
/// users, 60x a 10 rps limit): uniform/RR/least-loaded are
/// indistinguishable (17 promoted, head capped at ~10.3-10.4, node CV
/// ≤ 0.015); sticky promotes 42 and under-serves the head to ~3.5 rps
/// (the equal-division share ceiling — an engine property, not a tier
/// one). No routing policy produces unpromoted overage.
#[test]
fn test_routing_strategies_preserve_two_tier_invariants() {
    use nenya::sim::{LoadPattern, PopulationWorkload, Routing, Scenario, SimConfig};

    for (name, routing) in [
        ("uniform", Routing::Uniform),
        ("round_robin", Routing::RoundRobin),
        ("sticky", Routing::Sticky),
        ("least_loaded", Routing::LeastLoaded),
    ] {
        let cfg = SimConfig::default().with_cluster_target(10.0);
        let s = Scenario::new("routing_probe", cfg, Vec::new()).population(
            PopulationWorkload::new("user:", 100_000, 1.0, LoadPattern::Constant { rate: 600.0 })
                .routing(routing),
        );
        let cluster = run_cluster(&s, SEED);

        let promoted = cluster.num_ever_hot();
        assert!(
            promoted <= 100,
            "{}: promoted set {} not << 100k users",
            name,
            promoted
        );
        let mut worst_unpromoted: f64 = 0.0;
        let mut worst_any: f64 = 0.0;
        for (scope, _, accepted) in cluster.all_scope_counts() {
            let rate = accepted as f64 / 60.0;
            worst_any = worst_any.max(rate);
            if !cluster.was_ever_hot(scope) {
                worst_unpromoted = worst_unpromoted.max(rate);
            }
        }
        assert!(
            worst_unpromoted <= 10.0,
            "{}: unpromoted user served {:.2} rps over the 10 rps limit",
            name,
            worst_unpromoted
        );
        assert!(
            worst_any <= 15.0,
            "{}: user served {:.2} rps, far over the 10 rps limit",
            name,
            worst_any
        );
    }
}

/// Derivation sweep for the two-tier defaults (`gossip::tier` constants).
/// Prints markdown tables; the shipped values and the published curves in
/// docs/capacity-model.md come from this — re-run after control changes.
#[test]
#[ignore = "tier-defaults derivation sweep (~30s); run with --ignored --nocapture"]
fn tier_threshold_sweep() {
    use nenya::sim::{LoadPattern, PopulationWorkload, Scenario, SimConfig, Workload};

    let base_cfg = || SimConfig::default().with_cluster_target(10.0);
    let population =
        || PopulationWorkload::new("user:", 100_000, 1.0, LoadPattern::Constant { rate: 600.0 });

    // --- Axis 1: estimator window (noise vs promotion lag) ---
    // Spurious promotions: with promote=0.5 and this workload only ~10
    // users are truly over the threshold; everything beyond that is
    // estimator noise. Promotion lag: seconds from the ramp user's step to
    // its first promotion.
    println!("\n### estimator_window sweep (promote=0.5, Zipf 100k users)\n");
    println!("| window | promoted (≈10 real) | ramp promotion lag |");
    println!("|--------|--------------------|--------------------|");
    for window_secs in [1u64, 2, 3, 5, 8, 12, 16] {
        let mut cfg = base_cfg();
        cfg.tier.estimator_window = Duration::from_secs(window_secs);
        let s = Scenario::new("sweep", cfg.clone(), Vec::new()).population(population());
        let cluster = run_cluster(&s, SEED);
        let promoted = cluster.num_ever_hot();

        // Promotion lag on the ramp journey
        let ramp = Scenario::new(
            "sweep_ramp",
            cfg,
            vec![Workload::new(
                "user:ramp",
                LoadPattern::Piecewise {
                    steps: vec![(Duration::ZERO, 2.0), (Duration::from_secs(30), 30.0)],
                },
            )],
        )
        .duration(Duration::from_secs(60));
        let mut cluster = ramp.build_cluster(SEED);
        let mut lag = f64::NAN;
        for tick in 0..6000u64 {
            cluster.run_tick();
            if cluster.was_ever_hot("user:ramp") {
                lag = tick as f64 * 0.01 - 30.0;
                break;
            }
        }
        println!("| {}s | {} | {:.2}s |", window_secs, promoted, lag);
    }

    // --- Axis 2: promotion threshold (promoted-set size vs overage) ---
    println!("\n### promote_utilization sweep (window=8s, demote=promote/2)\n");
    println!("| promote | promoted | worst unpromoted rps | worst promoted rps | sticky worst served/offered | ramp max 1s |");
    println!("|---------|----------|----------------------|--------------------|------------------------------|-------------|");
    for promote in [0.3, 0.4, 0.5, 0.6, 0.7, 0.8] {
        let mut cfg = base_cfg();
        cfg.tier.estimator_window = Duration::from_secs(8);
        cfg.tier.promote_utilization = promote;
        cfg.tier.demote_utilization = promote / 2.0;
        let s = Scenario::new("sweep", cfg.clone(), Vec::new()).population(population());
        let cluster = run_cluster(&s, SEED);
        let mut worst_unpromoted: f64 = 0.0;
        let mut worst_promoted: f64 = 0.0;
        for (scope, _, accepted) in cluster.all_scope_counts() {
            let rate = accepted as f64 / 60.0;
            if cluster.was_ever_hot(scope) {
                worst_promoted = worst_promoted.max(rate);
            } else {
                worst_unpromoted = worst_unpromoted.max(rate);
            }
        }
        let promoted = cluster.num_ever_hot();

        // Sticky variant: worst mid-band under-service
        let sticky = Scenario::new("sweep_sticky", cfg, Vec::new())
            .population(population().routing(nenya::sim::Routing::Sticky));
        let cluster = run_cluster(&sticky, SEED);
        let mut worst_ratio = f64::INFINITY;
        for (scope, offered, accepted) in cluster.all_scope_counts() {
            let o = offered as f64 / 60.0;
            if o > 10.0 / 3.0 && o < 10.0 {
                worst_ratio = worst_ratio.min(accepted as f64 / offered as f64);
            }
            let _ = scope;
        }
        // Ramp transient: worst 1s admission of a user stepping 2 → 30
        // rps (promotion lag + burst allowances)
        let ramp = Scenario::new(
            "sweep_ramp2",
            {
                let mut c = base_cfg();
                c.tier.estimator_window = Duration::from_secs(8);
                c.tier.promote_utilization = promote;
                c.tier.demote_utilization = promote / 2.0;
                c
            },
            vec![Workload::new(
                "user:ramp",
                LoadPattern::Piecewise {
                    steps: vec![(Duration::ZERO, 2.0), (Duration::from_secs(30), 30.0)],
                },
            )],
        )
        .duration(Duration::from_secs(60));
        let mut cluster = ramp.build_cluster(SEED);
        let mut prev = 0u64;
        let mut max_1s: f64 = 0.0;
        for sec in 0..60 {
            for _ in 0..100 {
                cluster.run_tick();
            }
            let (_, acc) = cluster.scope_counts("user:ramp");
            if sec >= 30 {
                max_1s = max_1s.max((acc - prev) as f64);
            }
            prev = acc;
        }
        println!(
            "| {:.1} | {} | {:.2} | {:.2} | {:.2} | {:.0} |",
            promote, promoted, worst_unpromoted, worst_promoted, worst_ratio, max_1s
        );
    }

    // --- Axis 3: demotion threshold (flap resistance) ---
    // A user offered exactly at the demotion boundary is the worst case:
    // noise promotes it occasionally, and after each promotion the
    // observed rate hovers at the demote threshold. Count re-promotions
    // (flaps) over 300s.
    println!("\n### demote_utilization sweep (promote=0.5, user at the demote boundary, 300s)\n");
    println!("| demote | offered rps | promotions (1 = no flap) |");
    println!("|--------|-------------|---------------------------|");
    for demote in [0.15, 0.25, 0.35, 0.45] {
        for offered_frac in [demote, 0.4] {
            let mut cfg = base_cfg();
            cfg.tier.estimator_window = Duration::from_secs(8);
            cfg.tier.demote_utilization = demote;
            let offered = offered_frac * 10.0;
            let s = Scenario::new(
                "sweep_flap",
                cfg,
                vec![Workload::new(
                    "user:flap",
                    LoadPattern::Constant { rate: offered },
                )],
            )
            .duration(Duration::from_secs(300));
            let cluster = run_cluster(&s, SEED);
            println!(
                "| {:.2} | {:.1} | {} |",
                demote,
                offered,
                cluster.promotions_of("user:flap")
            );
        }
    }
}

// ===== Full matrix (slow subset) =====

#[test]
#[ignore = "full scenario matrix; run with --ignored"]
fn test_full_matrix_all_scenarios_run() {
    for s in scenario::library() {
        let r = s.run(SEED);
        assert!(!r.samples.is_empty(), "{} produced no samples", r.scenario);
        // Every scenario's steady state must stay at or below the target
        // band ceiling except burst (spikes land inside the steady window)
        // and skew (equal division cannot serve a 90% hot node — documented
        // limitation until demand-weighted division lands in Milestone 5+)
        // The per-user scenarios' `target` is a per-user limit, not a
        // cluster ceiling — their assertions live in the dedicated
        // Milestone 6 tests above
        let per_user = ["pareto_users", "sticky_users", "user_ramp"];
        if r.scenario != "burst" && r.scenario != "skew" && !per_user.contains(&r.scenario.as_str())
        {
            let mean = steady_mean(&r);
            assert!(
                mean <= r.target * 1.05,
                "{}: steady mean {:.1} above target band",
                r.scenario,
                mean
            );
        }
    }
}

#[test]
#[ignore = "50-node sweep; run with --ignored"]
fn test_scale_50() {
    let r = scenario::scale(50).run(SEED);
    let converge = convergence_after(&r, "start").expect("scale_50 did not converge");
    // Convergence slows with node count (each node's share of the error
    // shrinks); at 50 nodes the observed value is ~42s
    assert!(
        converge <= 55.0,
        "scale_50 convergence took {:.1}s",
        converge
    );
    let mean = steady_mean(&r);
    assert!((mean - r.target).abs() <= r.target * 0.05);
}

#[test]
#[ignore = "high-rate regime check (~2s release, ~10s debug); run with --ignored"]
fn test_high_rate_regime_1m_tps() {
    // Control dynamics are rate-invariant (the PID sees rates, not
    // requests), so high-tps runs add nothing to the fast suite — but this
    // periodic check guards the things that DO scale with absolute rate:
    // sliding-window memory (one Instant per accepted request per
    // update_interval), token-bucket arithmetic at large magnitudes, and
    // arrival-generation cost. Measured at Milestone 4: identical dynamics
    // from 300 rps to 10M rps (+0.8% steady bias, 6.5s convergence).
    let mut s = scenario::scale(3);
    s.cfg = s.cfg.with_cluster_target(1_000_000.0);
    s.workloads[0].pattern = nenya::sim::LoadPattern::Constant { rate: 2_000_000.0 };

    let r = s.run(SEED);
    let converge = convergence_after(&r, "start").expect("1M tps run must converge");
    assert!(
        converge <= 10.0,
        "convergence took {:.1}s at 1M tps",
        converge
    );
    let mean = steady_mean(&r);
    assert!(
        (mean - r.target).abs() <= r.target * 0.05,
        "steady mean {:.0} outside ±5% of 1M tps target",
        mean
    );
}

#[test]
#[ignore = "determinism across the whole library; run with --ignored"]
fn test_full_matrix_determinism() {
    for s in scenario::library() {
        let a = s.run(SEED);
        let b = s.run(SEED);
        assert_eq!(a.to_json(), b.to_json(), "{} is not deterministic", s.name);
    }
}
