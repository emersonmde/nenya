//! Gossip synchronization loop for distributed rate limiting

use super::aggregate::aggregate_peer_rates;
use super::GossipManager;
use crate::api::RateLimitManager;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "server")]
use tokio::sync::RwLock;
#[cfg(feature = "server")]
use tokio::time::interval;

/// Run the gossip synchronization loop
///
/// Every `sync_interval` this background task:
/// 1. Reads peer observations and aggregates them with age-weighted
///    staleness decay: peers whose state hasn't changed within
///    `2 × sync_interval` decay linearly to zero at `stale_timeout` and are
///    dropped past it (no manager lock)
/// 2. Under one write lock, applies the observations to the manager
///    ([`RateLimitManager::apply_peer_observations`]): stamps the live peer
///    count, feeds hot limiters their per-peer observations (zeroing scopes
///    no live peer reports so a vanished peer's last rate cannot linger as
///    phantom load), promotes tail scopes that peers gossip, runs demotion
///    hysteresis, and enforces the gossip budget — then refreshes limiter
///    state and collects the hot-tier rates to publish
///    ([`RateLimitManager::collect_gossip_rates`])
/// 3. Publishes the hot-tier rates to the cluster (no manager lock)
///
/// Only hot-tier scopes are gossiped; tail scopes are enforced locally at
/// their equal share and non-distributed scopes never participate. The
/// promotion/demotion policy lives in `gossip::tier` (transport-agnostic;
/// the simulator runs the same code).
#[cfg(feature = "server")]
pub async fn gossip_sync_loop(
    manager: Arc<RwLock<RateLimitManager>>,
    gossip: Arc<GossipManager>,
    _node_id: String,
    sync_interval: Duration,
    stale_timeout: Duration,
) {
    let mut tick = interval(sync_interval);

    loop {
        tick.tick().await;

        // 1. Aggregate peer rates with staleness decay (no manager lock)
        let observations = gossip.get_peer_observations().await;
        let aggregated = aggregate_peer_rates(&observations, sync_interval, stale_timeout);

        // 2. Apply observations + tier maintenance + collect publish set
        // (single write lock, no I/O inside)
        let (local_rates, tail_rates) = {
            let mut mgr = manager.write().await;
            let now = std::time::Instant::now();
            mgr.apply_peer_observations(&observations, &aggregated, now);
            let payload = mgr.collect_gossip_rates(now);
            tracing::debug!(
                "Gossip sync: {} scopes ({} hot), {} live peers, {} peer scopes",
                mgr.num_scopes(),
                mgr.num_hot_scopes(),
                aggregated.live_peers,
                aggregated.scope_rates.len()
            );
            payload
        };

        // 3. Publish hot-tier rates + per-pattern tail aggregates to the
        // cluster (manager lock released)
        if let Err(e) = gossip.publish_rates(&local_rates, &tail_rates).await {
            tracing::error!("Failed to publish gossip rates: {}", e);
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::api::RateLimitManager;

    #[tokio::test]
    async fn test_gossip_sync_loop_compiles() {
        // This test just verifies the sync loop compiles and can be called
        // Real testing happens in integration tests with multiple nodes

        let manager = Arc::new(RwLock::new(RateLimitManager::new(100.0, 0.8, 0.05, 0.04)));

        let gossip = Arc::new(
            GossipManager::new(
                "test-node".to_string(),
                "127.0.0.1:11000".parse().unwrap(),
                vec![],
                "test-cluster".to_string(),
            )
            .await
            .expect("Failed to create gossip manager"),
        );

        // Spawn sync loop in background
        let manager_clone = manager.clone();
        let gossip_clone = gossip.clone();
        let handle = tokio::spawn(async move {
            gossip_sync_loop(
                manager_clone,
                gossip_clone,
                "test-node".to_string(),
                Duration::from_millis(500),
                Duration::from_secs(10),
            )
            .await;
        });

        // Let it run for a short time
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Abort the loop
        handle.abort();

        // Clean shutdown (if possible - test might have multiple refs)
        if let Ok(gossip) = Arc::try_unwrap(gossip) {
            let _ = gossip.shutdown().await;
        }
    }

    #[tokio::test]
    async fn test_sync_loop_clears_external_rate_without_peers() {
        // A hot-tier limiter with a previously injected external rate must
        // be reset to zero by the sync loop when no live peers report that
        // scope
        let mut mgr = RateLimitManager::new(100.0, 0.8, 0.05, 0.04);
        let mut pattern = crate::api::ScopePattern::default_pattern(100.0);
        pattern.distributed = true;
        mgr.set_default_pattern(pattern);

        // Drive enough accepted traffic inside one estimator window to
        // cross the promotion threshold (0.5 × 100 rps with no peers)
        let start = std::time::Instant::now();
        for i in 0..80 {
            mgr.should_throttle_at("test-scope", start + Duration::from_millis(i * 10));
        }
        assert_eq!(mgr.scope_tier("test-scope"), Some("hot"));

        mgr.get_limiter_mut("test-scope")
            .unwrap()
            .set_external_accepted_request_rate(500.0);
        mgr.get_limiter_mut("test-scope").unwrap().set_num_peers(3);

        let manager = Arc::new(RwLock::new(mgr));

        let gossip = Arc::new(
            GossipManager::new(
                "test-node".to_string(),
                "127.0.0.1:11001".parse().unwrap(),
                vec![],
                "test-cluster".to_string(),
            )
            .await
            .expect("Failed to create gossip manager"),
        );

        let manager_clone = manager.clone();
        let gossip_clone = gossip.clone();
        let handle = tokio::spawn(async move {
            gossip_sync_loop(
                manager_clone,
                gossip_clone,
                "test-node".to_string(),
                Duration::from_millis(50),
                Duration::from_secs(10),
            )
            .await;
        });

        // Give the loop a few ticks
        tokio::time::sleep(Duration::from_millis(300)).await;
        handle.abort();

        let mgr = manager.read().await;
        let limiter = mgr.get_limiter("test-scope").unwrap();
        assert_eq!(limiter.external_accepted_request_rate(), 0.0);
        assert_eq!(limiter.num_peers(), 0);
    }
}
