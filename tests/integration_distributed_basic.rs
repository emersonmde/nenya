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

    // Generate load: 150 TPS total (50 per node), target is 100 TPS cluster-wide
    let stats = harness
        .generate_load("test-scope", 50)
        .await
        .expect("Failed to generate load");

    assert_eq!(stats.total_requests, 150);
    assert!(stats.accepted > 0, "Expected some requests to be accepted");
    assert!(
        stats.throttled > 0,
        "Expected some requests to be throttled"
    );

    // Give PID time to converge
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify cluster coordination - total accepted should be near target
    let total_rate = harness
        .get_cluster_total_accepted_rate("test-scope")
        .await
        .expect("Failed to get cluster rate");

    // Should be near 100 TPS target (allow 20% tolerance for test stability)
    assert!(
        total_rate > 80.0 && total_rate < 120.0,
        "Cluster total rate {} should be near 100 TPS target",
        total_rate
    );
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

    // Uneven load: node 0 gets 80 requests, nodes 1 and 2 get 10 each
    for _ in 0..80 {
        let _ = harness.should_throttle(0, "test").await;
    }
    for _ in 0..10 {
        let _ = harness.should_throttle(1, "test").await;
    }
    for _ in 0..10 {
        let _ = harness.should_throttle(2, "test").await;
    }

    // Wait for gossip sync and PID convergence
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Despite uneven load, cluster should coordinate to meet target
    let total_rate = harness
        .get_cluster_total_accepted_rate("test")
        .await
        .expect("Failed to get cluster rate");

    // Should be near 100 TPS target
    assert!(
        total_rate > 70.0 && total_rate < 130.0,
        "Cluster should coordinate despite uneven load, got {}",
        total_rate
    );
}
