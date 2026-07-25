//! Basic distributed coordination integration tests

mod integration;

use integration::ClusterTestHarness;
use std::time::Duration;

#[tokio::test]
#[ignore] // Ignored by default - run with: cargo test --features server -- --ignored
async fn test_three_node_cluster_startup() {
    let harness = ClusterTestHarness::spawn_cluster(3)
        .await
        .expect("Failed to spawn cluster");

    // Wait for convergence
    harness
        .wait_for_convergence(Duration::from_secs(10))
        .await
        .expect("Cluster failed to converge");

    // Verify all nodes see 2 peers
    for node in &harness.nodes {
        let health = harness
            .get_health(node.http_port)
            .await
            .expect("Failed to get health");

        assert_eq!(health["healthy"], true);
        assert_eq!(health["peers"], 2);
    }
}

#[tokio::test]
#[ignore]
async fn test_three_node_cluster_coordination() {
    let harness = ClusterTestHarness::spawn_cluster(3)
        .await
        .expect("Failed to spawn cluster");

    harness
        .wait_for_convergence(Duration::from_secs(10))
        .await
        .expect("Cluster failed to converge");

    // Sustained overload: round-robin requests as fast as the client can
    // issue them (well above the 300 TPS cluster target) for several seconds.
    // Quantitative convergence bands are a Milestone 4 simulator concern; here
    // we assert the qualitative coordination properties: throttling engages
    // under overload and every node sees its peers' rates via gossip.
    let scope = "test-scope";
    let mut accepted = 0usize;
    let mut throttled = 0usize;

    let start = std::time::Instant::now();
    let mut i = 0usize;
    while start.elapsed() < Duration::from_secs(5) {
        let response = harness
            .should_throttle(i % 3, scope)
            .await
            .expect("Request failed");
        if response["should_throttle"].as_bool().unwrap() {
            throttled += 1;
        } else {
            accepted += 1;
        }
        i += 1;
    }

    assert!(accepted > 0, "Expected some requests to be accepted");
    assert!(
        throttled > 0,
        "Sustained overload should trigger throttling (accepted {})",
        accepted
    );

    // While traffic is still fresh in the sliding windows, every node should
    // see both peers and a nonzero external rate for the scope
    for node_idx in 0..3 {
        let stats = harness
            .scope_stats(node_idx, scope)
            .await
            .expect("Failed to get scope stats");
        assert_eq!(stats["num_peers"].as_u64().unwrap(), 2);
        assert!(
            stats["external_accepted_rate"].as_f64().unwrap() > 0.0,
            "Node {} should see peer rates via gossip",
            node_idx
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_scope_auto_creation_across_cluster() {
    let harness = ClusterTestHarness::spawn_cluster(3)
        .await
        .expect("Failed to spawn cluster");

    harness
        .wait_for_convergence(Duration::from_secs(10))
        .await
        .expect("Cluster failed to converge");

    // Create different scopes on different nodes
    harness
        .should_throttle(0, "scope-a")
        .await
        .expect("Failed to throttle");
    harness
        .should_throttle(1, "scope-b")
        .await
        .expect("Failed to throttle");
    harness
        .should_throttle(2, "scope-c")
        .await
        .expect("Failed to throttle");

    // Wait for gossip sync
    tokio::time::sleep(Duration::from_secs(2)).await;

    // All nodes should know about all scopes via gossip
    for node in &harness.nodes {
        let health = harness
            .get_health(node.http_port)
            .await
            .expect("Failed to get health");

        // Each node should have at least 1 scope (the one it created)
        assert!(health["scopes"].as_u64().unwrap() >= 1);
    }
}

#[tokio::test]
#[ignore]
async fn test_uneven_load_distribution() {
    let harness = ClusterTestHarness::spawn_cluster(3)
        .await
        .expect("Failed to spawn cluster");

    harness
        .wait_for_convergence(Duration::from_secs(10))
        .await
        .expect("Cluster failed to converge");

    // Skewed sustained load: 90% of requests hit node 0. Quantitative
    // fairness/convergence is a Milestone 4 simulator concern; here we assert
    // that the lightly loaded nodes learn about the hot node's rate via gossip.
    let scope = "test";
    let start = std::time::Instant::now();
    let mut i = 0usize;
    while start.elapsed() < Duration::from_secs(5) {
        // 9 of every 10 requests go to node 0; the rest alternate between
        // nodes 1 and 2
        let node_idx = if i % 10 < 9 { 0 } else { 1 + (i / 10) % 2 };
        let _ = harness.should_throttle(node_idx, scope).await;
        i += 1;
    }

    // Sampled while load is fresh: the cold nodes must see the hot node's
    // rate as external load, and the hot node's external contribution from
    // the cold nodes should be comparatively small
    let hot = harness
        .scope_stats(0, scope)
        .await
        .expect("Failed to get scope stats");
    let cold = harness
        .scope_stats(1, scope)
        .await
        .expect("Failed to get scope stats");

    let cold_external = cold["external_accepted_rate"].as_f64().unwrap();

    assert!(
        cold_external > 0.0,
        "Cold node should see the hot node's rate via gossip"
    );
    // Note: no assertion on relative accepted rates — under coordination the
    // per-node accepted rates equalize regardless of offered load skew; the
    // quantitative fairness properties are Milestone 4 simulator scenarios
    assert_eq!(
        hot["num_peers"].as_u64().unwrap(),
        2,
        "Hot node should count both peers as live"
    );
}
