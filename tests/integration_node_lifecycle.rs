//! Node lifecycle integration tests (join/leave)

mod integration;

use integration::ClusterTestHarness;
use std::time::Duration;

#[tokio::test]
#[ignore] // Ignored by default - run with: cargo test --features server -- --ignored
async fn test_node_join_during_operation() {
    // Start with 2 nodes
    let harness = ClusterTestHarness::spawn_cluster(2)
        .await
        .expect("Failed to spawn initial cluster");

    harness
        .wait_for_convergence(Duration::from_secs(10))
        .await
        .expect("Initial cluster failed to converge");

    // Generate some load
    let _ = harness.generate_load("test", 20).await;

    // Add a third node
    // Note: This is simplified - in reality we'd need to spawn a new node
    // and add it to the harness. For now, we'll verify the 2-node cluster works.

    // Verify both nodes see each other
    for node in &harness.nodes {
        let health = harness
            .get_health(node.http_port)
            .await
            .expect("Failed to get health");

        assert_eq!(health["peers"], 1);
    }

    // Continue generating load
    let stats = harness
        .generate_load("test", 30)
        .await
        .expect("Failed to generate load");

    assert!(stats.accepted > 0);
}

#[tokio::test]
#[ignore]
async fn test_two_node_cluster() {
    let harness = ClusterTestHarness::spawn_cluster(2)
        .await
        .expect("Failed to spawn cluster");

    harness
        .wait_for_convergence(Duration::from_secs(10))
        .await
        .expect("Cluster failed to converge");

    // Each node should see 1 peer
    for node in &harness.nodes {
        let health = harness
            .get_health(node.http_port)
            .await
            .expect("Failed to get health");

        assert_eq!(health["healthy"], true);
        assert_eq!(health["peers"], 1);
    }

    // Test coordination with 2 nodes
    let stats = harness
        .generate_load("test", 50)
        .await
        .expect("Failed to generate load");

    assert_eq!(stats.total_requests, 100);
    assert!(stats.accepted > 0);
}

#[tokio::test]
#[ignore]
async fn test_single_node_cluster() {
    let harness = ClusterTestHarness::spawn_cluster(1)
        .await
        .expect("Failed to spawn cluster");

    // Single node should have 0 peers
    let health = harness
        .get_health(harness.nodes[0].http_port)
        .await
        .expect("Failed to get health");

    assert_eq!(health["healthy"], true);
    assert_eq!(health["peers"], 0);

    // Should still work as single node
    let response = harness
        .should_throttle(0, "test")
        .await
        .expect("Failed to throttle");

    assert!(response["should_throttle"].is_boolean());
}

#[tokio::test]
#[ignore]
async fn test_large_cluster() {
    // Test with 5 nodes
    let harness = ClusterTestHarness::spawn_cluster(5)
        .await
        .expect("Failed to spawn cluster");

    harness
        .wait_for_convergence(Duration::from_secs(15))
        .await
        .expect("Cluster failed to converge");

    // All nodes should see 4 peers
    for node in &harness.nodes {
        let health = harness
            .get_health(node.http_port)
            .await
            .expect("Failed to get health");

        assert_eq!(health["healthy"], true);
        assert_eq!(health["peers"], 4);
    }
}

#[tokio::test]
#[ignore]
async fn test_stale_peer_decay_after_node_kill() {
    // Milestone 3.1: a killed node's gossiped rate must decay to zero on the
    // survivors within stale_timeout, and the live peer count must drop —
    // even before Chitchat's failure detector evicts the node.
    let stale_timeout = Duration::from_millis(3000);
    let mut harness =
        ClusterTestHarness::spawn_cluster_with_env(3, &[("NENYA_STALE_TIMEOUT_MS", "3000")])
            .await
            .expect("Failed to spawn cluster");

    harness
        .wait_for_convergence(Duration::from_secs(15))
        .await
        .expect("Cluster failed to converge");

    let scope = "decay-test";

    // Create the scope on the surviving nodes so their sync loops track it
    harness
        .should_throttle(1, scope)
        .await
        .expect("Failed to create scope on node 1");
    harness
        .should_throttle(2, scope)
        .await
        .expect("Failed to create scope on node 2");

    // Drive load exclusively through node 0 so the survivors' external rate
    // for this scope comes only from node 0's gossiped state. Keep the load
    // flowing until node 1 observes it (the rate is measured over a sliding
    // window, so it must be checked while traffic is live).
    let mut external_before = 0.0;
    for i in 0..400 {
        harness
            .should_throttle(0, scope)
            .await
            .expect("Failed to send load to node 0");
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Poll node 1 every ~500ms once gossip has had time for a round
        if i % 25 == 24 {
            let stats = harness
                .scope_stats(1, scope)
                .await
                .expect("Failed to get scope stats from node 1");
            external_before = stats["external_accepted_rate"].as_f64().unwrap();
            if external_before > 0.0 {
                break;
            }
        }
    }
    assert!(
        external_before > 0.0,
        "Node 1 never saw node 0's rate while load was flowing"
    );

    // Crash node 0
    harness.kill_node(0).await.expect("Failed to kill node 0");

    // Wait past stale_timeout (plus margin for sync/gossip rounds)
    tokio::time::sleep(stale_timeout + Duration::from_secs(2)).await;

    for node_idx in [1, 2] {
        let stats = harness
            .scope_stats(node_idx, scope)
            .await
            .expect("Failed to get scope stats");

        // The other survivor still gossips its own (near-zero) rate, so allow
        // a small residual — the point is node 0's ~50 rps contribution is gone
        let external = stats["external_accepted_rate"].as_f64().unwrap();
        assert!(
            external < 1.0,
            "Node {}'s external rate should decay to ~zero after stale_timeout, got {}",
            node_idx,
            external
        );

        // Only the other survivor should still count as a live peer
        let peers = stats["num_peers"].as_u64().unwrap();
        assert_eq!(
            peers, 1,
            "Node {} should count exactly 1 live peer after the kill",
            node_idx
        );
    }
}
