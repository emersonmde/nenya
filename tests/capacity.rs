//! Capacity suite: scaling laws and cost coefficients (see
//! docs/capacity-model.md for the model these numbers feed).
//!
//! All tests are `#[ignore]`d — they are perf-validation rituals, not CI
//! gates. Run with:
//!
//! ```bash
//! cargo test --all-features --release --test capacity -- --ignored --nocapture
//! ```
//!
//! Release mode matters: the sweeps simulate hundreds of node-minutes.
//! Baselines in the assertions were measured at Milestone 4 (see
//! docs/capacity-model.md); loosen only with a documented reason.

#![cfg(feature = "sim")]

use std::time::Duration;

use nenya::sim::{scenario, LoadPattern};

/// Convergence time grows linearly with node count (~0.7s/node at the
/// production gains): equal division hands each node error/n of the cluster
/// error while gains stay fixed. Milestone 5 gain-scheduling targets this
/// law — if this test starts failing *fast*, that's the fix landing; update
/// the model doc.
#[test]
#[ignore = "capacity sweep (~5s release); run with --ignored"]
fn capacity_convergence_scales_linearly_with_nodes() {
    for (nodes, budget_secs) in [(50usize, 50.0f64), (100, 95.0)] {
        // Healthy per-node share (100 rps/node) so the estimator floor
        // (see below) doesn't confound the measurement
        let target = 100.0 * nodes as f64;
        let mut s = scenario::scale(nodes).duration(Duration::from_secs(300));
        s.cfg = s.cfg.with_cluster_target(target);
        s.workloads[0].pattern = LoadPattern::Constant { rate: target * 2.0 };
        let r = s.run(42);

        let converge = r.summary.convergence[0]
            .time_to_converge
            .unwrap_or_else(|| panic!("{} nodes did not converge in 300s", nodes));
        assert!(
            converge <= budget_secs,
            "{} nodes converged in {:.1}s (budget {:.0}s ≈ 0.9s/node; baseline ~0.75s/node)",
            nodes,
            converge,
            budget_secs
        );

        let n = r.samples.len();
        let steady = &r.samples[n - n / 4..];
        let mean = steady.iter().map(|s| s.accepted_rate).sum::<f64>() / steady.len() as f64;
        assert!(
            (mean - target).abs() <= target * 0.025,
            "{} nodes: steady mean {:.0} off target {:.0} (baseline bias <2%)",
            nodes,
            mean,
            target
        );
    }
}

/// Regression guard for the Milestone 5.4 estimator-floor fix: the
/// adaptive-window floor (`min_window_samples = 20`, swept in the roadmap's
/// estimator-floor item) keeps the sparse-share steady bias small. Before
/// the fix, the 1s fixed window mostly read empty at a few rps/node, the
/// PID chronically over-admitted, and this scenario (100 nodes sharing a
/// 300 rps target, 3 rps/node share) settled at +16% over target; with the
/// floor it measures +2.5%.
///
/// Also asserts the old defect stays reproducible with the floor disabled
/// (`min_window_samples = 0`), so this characterization keeps meaning
/// something if the estimator changes again.
#[test]
#[ignore = "capacity characterization (~4s release); run with --ignored"]
fn capacity_per_node_share_floor_fixed() {
    let steady_bias = |k: usize| {
        let mut s = scenario::scale(100).duration(Duration::from_secs(300));
        s.cfg = s.cfg.with_cluster_target(300.0); // 3 rps/node share
        s.cfg.min_window_samples = Some(k);
        s.workloads[0].pattern = LoadPattern::Constant { rate: 600.0 };
        let r = s.run(42);
        let n = r.samples.len();
        let steady = &r.samples[n - n / 4..];
        let mean = steady.iter().map(|s| s.accepted_rate).sum::<f64>() / steady.len() as f64;
        (mean - r.target) / r.target
    };

    let fixed = steady_bias(20);
    assert!(
        fixed < 0.04,
        "sparse-share bias with the adaptive-window floor measured {:+.1}% (baseline +2.5%)",
        fixed * 100.0
    );

    let legacy = steady_bias(0);
    assert!(
        legacy > 0.08,
        "fixed-window control run measured {:+.1}% — the characterization scenario \
         no longer reproduces the original defect; re-derive it",
        legacy * 100.0
    );
}

/// Cost coefficients for the capacity model: per-scope sync-loop CPU.
/// These bound the *CPU* cost of scope cardinality; the real binding
/// constraint today is gossip *bandwidth/encoding* (115 B/scope, single
/// blob — see the Milestone 6 per-scope-keys item), which the simulator
/// cannot measure.
#[cfg(feature = "server")]
#[test]
#[ignore = "coefficient microbench (~2s release); run with --ignored"]
fn capacity_scope_cost_coefficients() {
    use nenya::gossip::aggregate::{aggregate_peer_rates, PeerObservation};
    use nenya::gossip::GossipState;
    use std::collections::HashMap;
    use std::time::Instant;

    // Serialization: paid once per sync tick (every 500ms)
    let mut state = GossipState::new("node".to_string());
    for i in 0..10_000 {
        state.update_scope(format!("user:{:08x}", i), 123.45);
    }
    let wall = Instant::now();
    for _ in 0..20 {
        let _ = state.to_json().unwrap();
    }
    let per_publish = wall.elapsed() / 20;
    // Baseline ~0.9ms at 10k scopes (~88ns/scope); 5x headroom. Timing
    // baselines only mean something in release — in debug builds this test
    // reports coefficients without asserting them.
    let assert_timings = !cfg!(debug_assertions);
    if assert_timings {
        assert!(
            per_publish < Duration::from_millis(5),
            "serializing 10k scopes took {:?}/publish (baseline ~0.9ms)",
            per_publish
        );
    }

    // Aggregation: paid once per sync tick over peers × scopes
    let observations: Vec<PeerObservation> = (0..10)
        .map(|p| PeerObservation {
            node_id: format!("peer{}", p),
            age: Duration::from_millis(200),
            scope_rates: (0..10_000)
                .map(|i| (format!("user:{:08x}", i), 42.0))
                .collect::<HashMap<_, _>>(),
        })
        .collect();
    let wall = Instant::now();
    for _ in 0..10 {
        let _ = aggregate_peer_rates(
            &observations,
            Duration::from_millis(500),
            Duration::from_secs(10),
        );
    }
    let per_tick = wall.elapsed() / 10;
    // Baseline ~3.4ms for 10 peers × 10k scopes; 5x headroom
    if assert_timings {
        assert!(
            per_tick < Duration::from_millis(17),
            "aggregating 10 peers x 10k scopes took {:?}/tick (baseline ~3.4ms)",
            per_tick
        );
    }

    println!(
        "coefficients ({}): serialize {:?}/publish @10k scopes, aggregate {:?}/tick @10 peers x 10k scopes",
        if assert_timings {
            "release, asserted"
        } else {
            "debug build — informational only"
        },
        per_publish,
        per_tick
    );
}
