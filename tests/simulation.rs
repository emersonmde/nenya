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
//! - the initial ~1s overshoot spike in every scenario is the token buckets
//!   draining from full (production initializes buckets at capacity) and is
//!   deliberately not asserted against

#![cfg(feature = "sim")]

use nenya::sim::scenario;
use nenya::sim::RunResult;

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

    // Each joining node cold-starts with a full cluster-target-sized bucket
    // and admits a burst (~250 excess requests/join measured — see the
    // cold-start fair-share roadmap item). Bound the total; tighten this
    // when cold-start initialization is fixed.
    assert!(
        r.summary.integrated_overshoot < 12_000.0,
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
        if r.scenario != "burst" && r.scenario != "skew" {
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
