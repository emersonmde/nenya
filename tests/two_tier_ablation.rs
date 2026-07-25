//! Two-tier ablation (Milestone 6 follow-up): what would all-scopes-hot
//! cost now that the wire has per-scope keys + delta sync? Answers "is
//! promotion still needed" with a before/after measurement. The "before"
//! arm emulates no tail tier (promotion threshold ~0, unbounded budget).
//!
//! Measured 2026-07 (seed 42, M-series, release, 300k scopes, 2 peers):
//! - RSS: 356 B/scope tail vs 949 B/scope hot (engine + peer-observation
//!   vectors + timestamp deque)
//! - Steady sync tick (apply + collect, every 500 ms): 1.2 ms two-tier vs
//!   204 ms all-hot — 40% of the tick budget at 300k scopes and O(scopes
//!   × peers) under the manager write lock; the one-time promote pass is
//!   another 273 ms
//! - Publish set: 0 keys (nothing near limit) vs 300k keys ≈ 5.9 MB of
//!   replicated keyspace every node must hold per peer and every joiner
//!   must catch up (~1.5 min at 64 KB/round on Linux; stalls on default
//!   macOS — see gossip_wire.rs)
//! - Steady delta wire: ~400 value-changing scopes/s at 600 rps over
//!   100k Zipf users (~24 KB/s/peer) — modest, but proportional to
//!   distinct active users/sec, i.e. it scales with traffic; two-tier
//!   caps it at the published set
//!
//! Verdict: promotion is still required, but the binder moved — delta
//! sync fixed the original retransmit-everything cost; what remains is
//! sync-tick CPU, replicated keyspace + joiner catch-up, and
//! traffic-proportional churn. Re-run with
//! `cargo test --all-features --release --test two_tier_ablation -- --ignored --nocapture --test-threads=1`
#![cfg(all(feature = "server", feature = "sim"))]

use nenya::api::{RateLimitManager, ScopePattern};
use nenya::gossip::aggregate::{aggregate_peer_rates, PeerObservation};
use std::collections::HashMap;
use std::time::{Duration, Instant};

fn rss_kb() -> usize {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

fn manager(promote: f64, demote: f64, budget: usize) -> RateLimitManager {
    let mut mgr = RateLimitManager::new(10.0, 0.5, 0.02, 0.08);
    mgr.set_gossip_budget(budget);
    let mut pattern = ScopePattern::default_pattern(10.0);
    pattern.distributed = true;
    pattern.promote_utilization = Some(promote);
    pattern.demote_utilization = Some(demote);
    mgr.set_default_pattern(pattern);
    mgr
}

#[test]
#[ignore = "ablation measurement (~2s release, ~500MB RSS); run with --ignored --nocapture --test-threads=1"]
fn ablation_all_hot_memory_and_sync_cpu() {
    const N: usize = 300_000;
    // Peers reporting a scope IS the promotion evidence, so the arms are
    // distinguished by what peers report: liveness-only (two-tier reality:
    // peers publish just their small hot/watched sets) vs every scope
    // (the ablated all-hot world).
    for (name, promote, budget, peers_report_all) in [
        ("two-tier (default)", 0.5, 1000usize, false),
        ("all-hot (ablated)", 1e-9, usize::MAX, true),
    ] {
        let mut mgr = manager(promote, promote * 0.5, budget);
        let start = Instant::now();
        let rss_before = rss_kb();
        let wall = Instant::now();
        for i in 0..N {
            let now = start + Duration::from_nanos(i as u64 * 1000);
            let scope = format!("user:{:08x}", i);
            mgr.should_throttle_at(&scope, now);
            mgr.should_throttle_at(&scope, now + Duration::from_nanos(200));
        }
        let create = wall.elapsed();

        let peer_rates: HashMap<String, f64> = if peers_report_all {
            (0..N).map(|i| (format!("user:{:08x}", i), 3.3)).collect()
        } else {
            HashMap::new()
        };
        let observations: Vec<PeerObservation> = (0..2)
            .map(|p| PeerObservation {
                node_id: format!("peer{}", p),
                age: Duration::from_millis(200),
                scope_rates: peer_rates.clone(),
                tail_rates: HashMap::new(),
            })
            .collect();
        let aggregated = aggregate_peer_rates(
            &observations,
            Duration::from_millis(500),
            Duration::from_secs(10),
        );
        // First apply includes evidence-based promotion (one-time);
        // second apply is the steady per-tick cost
        let now = start + Duration::from_secs(2);
        let wall = Instant::now();
        mgr.apply_peer_observations(&observations, &aggregated, now);
        let apply_first = wall.elapsed();
        let wall = Instant::now();
        mgr.apply_peer_observations(&observations, &aggregated, now + Duration::from_millis(500));
        let apply = wall.elapsed();
        let wall = Instant::now();
        let (rates, tails) = mgr.collect_gossip_rates(now + Duration::from_secs(1));
        let collect = wall.elapsed();
        let rss_after = rss_kb();
        let publish_bytes: usize = rates
            .iter()
            .map(|(s, _)| "s:".len() + s.len() + "3.300".len())
            .sum();

        println!(
            "{}: {} scopes ({} hot), {} B/scope RSS, create {:.0} ns/scope, promote-pass {:?}, steady sync tick: apply={:?} collect={:?}, publish set {} keys (~{} KB), tails {}",
            name,
            mgr.num_scopes(),
            mgr.num_hot_scopes(),
            (rss_after.saturating_sub(rss_before)) * 1024 / N,
            create.as_nanos() as f64 / N as f64,
            apply_first,
            apply,
            collect,
            rates.len(),
            publish_bytes / 1024,
            tails.len(),
        );
    }
}
#[test]
#[ignore = "ablation measurement (~1s release); run with --ignored --nocapture"]
fn ablation_wire_churn_with_delta_sync() {
    // How many gossip values actually change per second (i.e., must be
    // retransmitted even with delta sync) if every active scope is hot?
    use nenya::sim::{LoadPattern, PopulationWorkload, Scenario, SimConfig};
    let cfg = SimConfig::default().with_cluster_target(10.0);
    let s = Scenario::new("churn", cfg, Vec::new()).population(PopulationWorkload::new(
        "user:",
        100_000,
        1.0,
        LoadPattern::Constant { rate: 600.0 },
    ));
    let mut cluster = s.build_cluster(42);
    let mut prev: HashMap<String, u64> = HashMap::new();
    let mut active_per_sec: Vec<usize> = Vec::new();
    for sec in 0..60 {
        for _ in 0..100 {
            cluster.run_tick();
        }
        if sec >= 10 {
            let mut active = 0usize;
            for (scope, _, accepted) in cluster.all_scope_counts() {
                if prev.get(scope).copied().unwrap_or(0) != accepted {
                    active += 1;
                }
            }
            active_per_sec.push(active);
        }
        prev = cluster
            .all_scope_counts()
            .map(|(s, _, a)| (s.clone(), a))
            .collect();
    }
    let mean = active_per_sec.iter().sum::<usize>() as f64 / active_per_sec.len() as f64;
    let max = active_per_sec.iter().max().unwrap();
    println!(
        "active (value-changing) scopes/sec at 600 rps over 100k Zipf users: mean {:.0}, max {} -> all-hot delta wire ~{:.0} KB/s/peer vs two-tier hot set 17 (~1 KB/s)",
        mean,
        max,
        mean * 31.0 * 2.0 / 1024.0
    );
}
