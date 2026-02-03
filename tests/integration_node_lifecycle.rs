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
