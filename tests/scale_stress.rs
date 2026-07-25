//! Memory-at-cardinality stress checks (Milestone 6.3): 1M+ tail scopes
//! with churn through the real `RateLimitManager`.
//!
//! Measured numbers are recorded in docs/capacity-model.md; re-run with
//! `cargo test --all-features --release --test scale_stress -- --ignored --nocapture`.

#![cfg(feature = "server")]

use nenya::api::{RateLimitManager, ScopePattern};
use nenya::gossip::aggregate::{aggregate_peer_rates, AggregatedRates};
use serial_test::serial;
use std::time::{Duration, Instant};

fn rss_kb() -> usize {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

fn distributed_manager() -> RateLimitManager {
    let mut mgr = RateLimitManager::new(300.0, 0.5, 0.02, 0.08);
    let mut pattern = ScopePattern::default_pattern(300.0);
    pattern.distributed = true;
    mgr.set_default_pattern(pattern);
    mgr
}

/// One million distinct per-user tail scopes: bytes/scope and creation
/// throughput. Target from the roadmap: 10⁶ tail scopes in the low
/// hundreds of MB.
#[test]
#[serial]
#[ignore = "1M-scope memory stress (~1s release, ~400MB RSS); RSS-based, so serialized; run with --ignored --nocapture"]
fn stress_one_million_tail_scopes() {
    const N: usize = 1_000_000;
    let mut mgr = distributed_manager();
    let start = Instant::now();

    // Component sizes for the record (enum must stay tail-sized)
    println!(
        "size_of ScopeEntry-equivalent: TailScope={}B",
        std::mem::size_of::<nenya::gossip::tier::TailScope>()
    );

    let rss_before = rss_kb();
    let wall = Instant::now();
    for i in 0..N {
        // One request per user, spread over a virtual second so no scope
        // approaches its promotion threshold
        let now = start + Duration::from_nanos((i as u64) * 1_000);
        let decision = mgr.should_throttle_at(&format!("user:{:08x}", i), now);
        assert!(!decision.should_throttle, "first request must admit");
    }
    let create_elapsed = wall.elapsed();
    let rss_after = rss_kb();

    let bytes_per_scope = (rss_after.saturating_sub(rss_before)) * 1024 / N;
    println!(
        "{} tail scopes: {} KB -> {} KB RSS, ~{} B/scope, {:.0} ns/create+admit",
        N,
        rss_before,
        rss_after,
        bytes_per_scope,
        create_elapsed.as_nanos() as f64 / N as f64
    );

    assert_eq!(mgr.num_scopes(), N);
    assert_eq!(mgr.num_hot_scopes(), 0, "all scopes must remain tail");
    // "Low hundreds of MB" target → ≤ 400 B/scope keeps 1M under ~400 MB
    assert!(
        bytes_per_scope <= 400,
        "tail scope footprint {} B/scope exceeds the 400 B budget",
        bytes_per_scope
    );

    // Warm-path check: another request on an existing tail scope stays fast
    let wall = Instant::now();
    for i in 0..10_000 {
        let now = start + Duration::from_secs(2) + Duration::from_nanos(i * 1_000);
        mgr.should_throttle_at(&format!("user:{:08x}", i), now);
    }
    println!(
        "warm tail admit: {:.0} ns/decision",
        wall.elapsed().as_nanos() as f64 / 10_000.0
    );
}

/// Churn: users go idle, TTL eviction reclaims them, new users replace
/// them — memory must be bounded by the active set, not the total ever
/// seen.
#[test]
#[serial]
#[ignore = "2M-user churn stress (~2s release); run with --ignored --nocapture"]
fn stress_scope_churn_with_ttl_eviction() {
    const WAVE: usize = 250_000;
    const WAVES: usize = 8; // 2M distinct users total
    let mut mgr = distributed_manager();
    mgr.set_scope_ttl(Duration::from_secs(60));
    let start = Instant::now();
    let empty = AggregatedRates::default();
    let _ = aggregate_peer_rates(&[], Duration::from_millis(500), Duration::from_secs(10));

    let mut peak_scopes = 0usize;
    for wave in 0..WAVES {
        // Each wave is 40 virtual seconds of traffic from a fresh user set
        let wave_base = start + Duration::from_secs(40 * wave as u64);
        for i in 0..WAVE {
            let user = wave * WAVE + i;
            let now = wave_base + Duration::from_nanos(i as u64 * 1_000);
            mgr.should_throttle_at(&format!("user:{:08x}", user), now);
        }
        // Sync-loop apply pass at the end of the wave triggers the TTL
        // sweep (users idle > 60s — two waves back — are evicted)
        let sweep_at = wave_base + Duration::from_secs(39);
        mgr.apply_peer_observations(&[], &empty, sweep_at);
        peak_scopes = peak_scopes.max(mgr.num_scopes());
    }

    println!(
        "churn: {} distinct users, peak resident scopes {}, final {}",
        WAVES * WAVE,
        peak_scopes,
        mgr.num_scopes()
    );

    // With a 60s TTL and 40s waves, at most ~2 waves can be resident at a
    // sweep; the peak may briefly hold 3 (current + 2 not yet swept)
    assert!(
        peak_scopes <= 3 * WAVE,
        "TTL eviction failed to bound the resident set: peak {} scopes",
        peak_scopes
    );
    assert!(
        mgr.num_scopes() <= 2 * WAVE,
        "final resident set {} not bounded by the TTL window",
        mgr.num_scopes()
    );
}
