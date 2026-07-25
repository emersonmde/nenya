//! Measures `/should_throttle` decision latency with the gossip sync loop
//! idle vs. active, at 1, 100, and 1000 scopes (Milestone 3.2).
//!
//! This exercises the same lock pattern as the HTTP handler: a write lock on
//! the shared `RateLimitManager` per decision, contending with the sync loop's
//! two write-lock passes per tick. Run with:
//!
//! ```bash
//! cargo bench --features server --bench gossip_contention_bench
//! ```

use nenya::api::RateLimitManager;
use nenya::gossip::{gossip_sync_loop, GossipManager};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const SCOPE_COUNTS: &[usize] = &[1, 100, 1000];
const WARMUP_ITERS: usize = 5_000;
const MEASURE_ITERS: usize = 100_000;

fn percentile(sorted_nanos: &[u64], p: f64) -> u64 {
    let idx = ((sorted_nanos.len() as f64 - 1.0) * p).round() as usize;
    sorted_nanos[idx]
}

async fn build_manager(num_scopes: usize) -> Arc<RwLock<RateLimitManager>> {
    let mut mgr = RateLimitManager::new(1_000_000.0, 0.8, 0.05, 0.04);
    for i in 0..num_scopes {
        mgr.should_throttle(&format!("scope-{}", i));
    }
    Arc::new(RwLock::new(mgr))
}

/// Measure per-decision latency through the shared-lock path, round-robining
/// across scopes like uniform HTTP traffic would.
async fn measure(manager: &Arc<RwLock<RateLimitManager>>, num_scopes: usize) -> Vec<u64> {
    let scopes: Vec<String> = (0..num_scopes).map(|i| format!("scope-{}", i)).collect();

    for i in 0..WARMUP_ITERS {
        let mut mgr = manager.write().await;
        mgr.should_throttle(&scopes[i % num_scopes]);
    }

    let mut latencies = Vec::with_capacity(MEASURE_ITERS);
    for i in 0..MEASURE_ITERS {
        let scope = &scopes[i % num_scopes];
        let start = Instant::now();
        {
            let mut mgr = manager.write().await;
            mgr.should_throttle(scope);
        }
        latencies.push(start.elapsed().as_nanos() as u64);
    }
    latencies.sort_unstable();
    latencies
}

fn report(label: &str, num_scopes: usize, sorted_nanos: &[u64]) {
    println!(
        "{:<14} {:>6} scopes  p50={:>7}ns  p99={:>8}ns  p99.9={:>8}ns  max={:>10}ns",
        label,
        num_scopes,
        percentile(sorted_nanos, 0.50),
        percentile(sorted_nanos, 0.99),
        percentile(sorted_nanos, 0.999),
        sorted_nanos[sorted_nanos.len() - 1],
    );
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    println!(
        "gossip contention benchmark: {} iters/case after {} warmup",
        MEASURE_ITERS, WARMUP_ITERS
    );

    // Distinct gossip port per case (not a loop counter — cases consume one
    // port each and the loop below binds two managers per iteration)
    let mut port = 13100u16;
    #[allow(clippy::explicit_counter_loop)]
    for &num_scopes in SCOPE_COUNTS {
        // Baseline: no gossip loop running
        let manager = build_manager(num_scopes).await;
        let idle = measure(&manager, num_scopes).await;
        report("gossip-idle", num_scopes, &idle);

        // Active: real sync loop against a real (peerless) gossip manager,
        // ticking at the production 500ms interval
        let manager = build_manager(num_scopes).await;
        let gossip = Arc::new(
            GossipManager::new(
                "bench-node".to_string(),
                format!("127.0.0.1:{}", port).parse().unwrap(),
                vec![],
                "bench-cluster".to_string(),
            )
            .await
            .expect("failed to create gossip manager"),
        );
        port += 1;

        let sync_handle = tokio::spawn(gossip_sync_loop(
            manager.clone(),
            gossip.clone(),
            "bench-node".to_string(),
            Duration::from_millis(500),
            Duration::from_secs(10),
        ));

        let active = measure(&manager, num_scopes).await;
        report("gossip-active", num_scopes, &active);
        sync_handle.abort();
        println!();
    }
}
