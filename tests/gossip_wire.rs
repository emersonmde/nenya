//! Real-UDP gossip wire-format verification (Milestone 6.1).
//!
//! The simulator's abstract message bus cannot answer whether chitchat's
//! MTU-bounded UDP anti-entropy actually propagates a large per-scope key
//! set — this test can: two real `GossipManager`s (real chitchat, real UDP
//! sockets on loopback) exchange 10k scope keys and we measure
//! time-to-full-propagation.
//!
//! Findings are recorded in docs/capacity-model.md; re-run with
//! `cargo test --all-features --test gossip_wire -- --ignored --nocapture`.

#![cfg(feature = "server")]

use nenya::gossip::GossipManager;
use std::time::{Duration, Instant};

/// 10k scope keys, introduced incrementally (the realistic pattern: scopes
/// promote into the hot tier over time; the steady-state delta is only the
/// keys whose rounded rate changed).
///
/// **Platform finding (measured 2026-07, macOS 15 / chitchat 0.10):**
/// chitchat builds anti-entropy deltas up to its hardcoded 65 507 B UDP
/// datagram limit. On Linux such datagrams IP-fragment and a backlog drains
/// at ~64 KB per 1 s gossip round per peer. On macOS the default
/// `net.inet.udp.maxdgram` is 9216, the oversized `sendto` fails with
/// EMSGSIZE, and — because chitchat always rebuilds the *largest* delta —
/// a node whose pending delta ever exceeds ~9 KB stalls **forever** (0 of
/// 10k keys after 60 s, while phi-accrual liveness stays green). A >300-key
/// burst (e.g. a node joining a cluster with a large hot set) hits this on
/// unmodified macOS; `sudo sysctl -w net.inet.udp.maxdgram=65535` clears
/// it. Incremental growth below ~9 KB/round propagates fine on both
/// platforms, which is what this test asserts.
#[tokio::test]
#[ignore = "real-UDP propagation check (~2min); run with --ignored --nocapture"]
async fn test_10k_scope_keys_propagate_incrementally_over_real_udp() {
    let addr_a = "127.0.0.1:19801".parse().unwrap();
    let addr_b = "127.0.0.1:19802".parse().unwrap();

    let node_a = GossipManager::new(
        "wire-node-a".to_string(),
        addr_a,
        vec![],
        "wire-test".to_string(),
    )
    .await
    .expect("node A");
    let node_b = GossipManager::new(
        "wire-node-b".to_string(),
        addr_b,
        vec![addr_a.to_string()],
        "wire-test".to_string(),
    )
    .await
    .expect("node B");

    const NUM_SCOPES: usize = 10_000;
    // ~31 B/key on the chitchat wire (key + value + version overhead);
    // 100 new keys per 500 ms publish ≈ 6.2 KB/s of delta — under the
    // 9216 B/round macOS ceiling, far under the 65 KB/round Linux one
    const KEYS_PER_PUBLISH: usize = 100;
    let rates: Vec<(String, f64)> = (0..NUM_SCOPES)
        .map(|i| (format!("user:{:08x}", i), 42.5))
        .collect();
    let payload_bytes: usize = rates
        .iter()
        .map(|(scope, _)| "s:".len() + scope.len() + "42.500".len())
        .sum();

    let start = Instant::now();
    let deadline = start + Duration::from_secs(180);
    let mut published = 0usize;
    let mut seen = 0usize;
    while Instant::now() < deadline {
        published = (published + KEYS_PER_PUBLISH).min(NUM_SCOPES);
        node_a
            .publish_rates(&rates[..published])
            .await
            .expect("publish");
        tokio::time::sleep(Duration::from_millis(500)).await;
        let obs = node_b.get_peer_observations().await;
        seen = obs
            .iter()
            .find(|o| o.node_id == "wire-node-a")
            .map(|o| o.scope_rates.len())
            .unwrap_or(0);
        if start.elapsed().as_millis() % 10_000 < 500 {
            println!(
                "t={:?}: A published {}, B sees {}",
                start.elapsed(),
                published,
                seen
            );
        }
        if seen == NUM_SCOPES {
            break;
        }
    }
    let elapsed = start.elapsed();
    println!(
        "propagated {}/{} scope keys ({} payload bytes) in {:?}",
        seen, NUM_SCOPES, payload_bytes, elapsed
    );

    assert_eq!(
        seen, NUM_SCOPES,
        "10k incrementally-published scope keys must fully propagate over \
         real UDP (got {} — if this is ~0 on macOS, see the datagram-size \
         note above)",
        seen
    );

    // Values arrive intact
    let obs = node_b.get_peer_observations().await;
    let a_obs = obs.iter().find(|o| o.node_id == "wire-node-a").unwrap();
    assert_eq!(a_obs.scope_rates["user:00000000"], 42.5);

    let _ = node_b.shutdown().await;
    let _ = node_a.shutdown().await;
}

/// Deleted scope keys (hot → tail demotion) must disappear from the peer's
/// view — a demoted scope may not linger as phantom gossip state.
#[tokio::test]
#[ignore = "real-UDP deletion check (~90s: waits out the 60s tombstone grace); run with --ignored"]
async fn test_deleted_scope_keys_vanish_from_peer_view() {
    let addr_a = "127.0.0.1:19811".parse().unwrap();
    let addr_b = "127.0.0.1:19812".parse().unwrap();

    let node_a = GossipManager::new(
        "wire-node-a".to_string(),
        addr_a,
        vec![],
        "wire-test-del".to_string(),
    )
    .await
    .expect("node A");
    let node_b = GossipManager::new(
        "wire-node-b".to_string(),
        addr_b,
        vec![addr_a.to_string()],
        "wire-test-del".to_string(),
    )
    .await
    .expect("node B");

    let both = vec![("keep".to_string(), 10.0), ("drop".to_string(), 20.0)];
    let keep_only = vec![("keep".to_string(), 10.0)];

    // Publish both scopes until B sees them
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        node_a.publish_rates(&both).await.expect("publish");
        tokio::time::sleep(Duration::from_millis(500)).await;
        let obs = node_b.get_peer_observations().await;
        if obs
            .iter()
            .any(|o| o.node_id == "wire-node-a" && o.scope_rates.len() == 2)
        {
            break;
        }
        assert!(Instant::now() < deadline, "initial propagation timed out");
    }

    // Demote `drop`: the key is deleted and must vanish at B
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        node_a.publish_rates(&keep_only).await.expect("republish");
        tokio::time::sleep(Duration::from_millis(500)).await;
        let obs = node_b.get_peer_observations().await;
        let a_obs = obs.iter().find(|o| o.node_id == "wire-node-a");
        if let Some(a_obs) = a_obs {
            if !a_obs.scope_rates.contains_key("drop") {
                assert!(a_obs.scope_rates.contains_key("keep"));
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "deleted scope key still visible at peer after 60s"
        );
    }

    let _ = node_b.shutdown().await;
    let _ = node_a.shutdown().await;
}
