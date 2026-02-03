//! Distributed load testing integration tests

mod integration;

use integration::ClusterTestHarness;
use std::time::Duration;

#[tokio::test]
#[ignore] // Ignored by default - run with: cargo test --features server -- --ignored
async fn test_sustained_load() {
    let harness = ClusterTestHarness::spawn_cluster(3)
        .await
        .expect("Failed to spawn cluster");

    harness
        .wait_for_convergence(Duration::from_secs(10))
        .await
        .expect("Cluster failed to converge");

    // Generate sustained load over multiple rounds
    let mut total_accepted = 0;
    let mut total_throttled = 0;

    for _ in 0..5 {
        let stats = harness
            .generate_load("test", 30)
            .await
            .expect("Failed to generate load");

        total_accepted += stats.accepted;
        total_throttled += stats.throttled;

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Should see both accepted and throttled requests
    assert!(total_accepted > 0, "Expected some requests to be accepted");
    assert!(
        total_throttled > 0,
        "Expected some requests to be throttled"
    );

    // Total should be 450 requests (5 rounds * 3 nodes * 30 requests)
    assert_eq!(total_accepted + total_throttled, 450);
}

#[tokio::test]
#[ignore]
async fn test_burst_load() {
    let harness = ClusterTestHarness::spawn_cluster(3)
        .await
        .expect("Failed to spawn cluster");

    harness
        .wait_for_convergence(Duration::from_secs(10))
        .await
        .expect("Cluster failed to converge");

    // Large burst of requests
    let stats = harness
        .generate_load("test", 100)
        .await
        .expect("Failed to generate load");

    assert_eq!(stats.total_requests, 300);

    // Most should be throttled in a burst
    assert!(
        stats.throttled > stats.accepted,
        "Burst should result in more throttled than accepted"
    );
}

#[tokio::test]
#[ignore]
async fn test_multiple_scopes_parallel() {
    let harness = ClusterTestHarness::spawn_cluster(3)
        .await
        .expect("Failed to spawn cluster");

    harness
        .wait_for_convergence(Duration::from_secs(10))
        .await
        .expect("Cluster failed to converge");

    // Generate load on multiple scopes simultaneously
    let scope_a = harness.generate_load("scope-a", 20);
    let scope_b = harness.generate_load("scope-b", 20);
    let scope_c = harness.generate_load("scope-c", 20);

    let (stats_a, stats_b, stats_c) = tokio::join!(scope_a, scope_b, scope_c);

    let stats_a = stats_a.expect("Failed scope-a");
    let stats_b = stats_b.expect("Failed scope-b");
    let stats_c = stats_c.expect("Failed scope-c");

    // All scopes should have some activity
    assert!(stats_a.accepted > 0);
    assert!(stats_b.accepted > 0);
    assert!(stats_c.accepted > 0);

    // Total requests should be 180 (3 scopes * 3 nodes * 20 requests)
    assert_eq!(
        stats_a.total_requests + stats_b.total_requests + stats_c.total_requests,
        180
    );
}

#[tokio::test]
#[ignore]
async fn test_gradual_load_increase() {
    let harness = ClusterTestHarness::spawn_cluster(3)
        .await
        .expect("Failed to spawn cluster");

    harness
        .wait_for_convergence(Duration::from_secs(10))
        .await
        .expect("Cluster failed to converge");

    // Gradually increase load
    for requests_per_node in [5, 10, 20, 40] {
        let stats = harness
            .generate_load("test", requests_per_node)
            .await
            .expect("Failed to generate load");

        assert_eq!(stats.total_requests, requests_per_node * 3);

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // Verify cluster is still coordinating
    let health = harness
        .get_health(harness.nodes[0].http_port)
        .await
        .expect("Failed to get health");

    assert_eq!(health["healthy"], true);
    assert_eq!(health["peers"], 2);
}

#[tokio::test]
#[ignore]
async fn test_concentrated_load_on_one_node() {
    let harness = ClusterTestHarness::spawn_cluster(3)
        .await
        .expect("Failed to spawn cluster");

    harness
        .wait_for_convergence(Duration::from_secs(10))
        .await
        .expect("Cluster failed to converge");

    // All load goes to node 0
    let mut total_accepted = 0;

    for _ in 0..100 {
        let response = harness
            .should_throttle(0, "test")
            .await
            .expect("Failed to throttle");

        if !response["should_throttle"].as_bool().unwrap() {
            total_accepted += 1;
        }
    }

    // Even though all load is on one node, cluster should coordinate
    // Node 0 should not accept all 100 requests
    assert!(
        total_accepted < 100,
        "Node should not accept all requests even if it receives all load"
    );

    // Wait for gossip
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Other nodes should be aware via gossip
    for i in 1..harness.nodes.len() {
        let health = harness
            .get_health(harness.nodes[i].http_port)
            .await
            .expect("Failed to get health");

        assert!(health["scopes"].as_u64().unwrap() >= 1);
    }
}
