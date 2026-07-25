//! Gossip synchronization loop for distributed rate limiting

use super::aggregate::aggregate_peer_rates;
use super::{GossipManager, GossipState};
use crate::api::RateLimitManager;
use crate::engine::PeerRate;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "server")]
use tokio::sync::RwLock;
#[cfg(feature = "server")]
use tokio::time::interval;

/// Run the gossip synchronization loop
///
/// Every `sync_interval` this background task:
/// 1. Refreshes and collects local per-scope rates (short write lock)
/// 2. Publishes state to the cluster via gossip (no manager lock)
/// 3. Aggregates peer rates with age-weighted staleness decay: peers whose
///    state hasn't changed within `2 × sync_interval` decay linearly to zero
///    at `stale_timeout` and are dropped past it (no manager lock)
/// 4. Applies external rates and the live peer count to every limiter in a
///    single pass under one write lock; scopes with no live peer data have
///    their external rate reset to zero so a vanished peer's last known rate
///    cannot linger as phantom load
///
/// Lock behavior: the manager write lock is held twice per tick (once for the
/// local rate refresh, which must mutate limiter state, and once for the
/// update pass), each scoped to a single pass over the limiters with no I/O or
/// awaits inside. Gossip I/O happens between the two critical sections.
#[cfg(feature = "server")]
pub async fn gossip_sync_loop(
    manager: Arc<RwLock<RateLimitManager>>,
    gossip: Arc<GossipManager>,
    node_id: String,
    sync_interval: Duration,
    stale_timeout: Duration,
) {
    let mut tick = interval(sync_interval);

    loop {
        tick.tick().await;

        // 1. Collect local state with fresh rate calculations. This needs a
        // write lock because update_state_at advances limiter state.
        let local_state = {
            let mut mgr = manager.write().await;
            let mut state = GossipState::new(node_id.clone());

            let now = std::time::Instant::now();
            for (scope_name, limiter) in mgr.limiters_mut() {
                limiter.update_state_at(now);
                let fresh_rate = limiter.local_accepted_request_rate();
                tracing::debug!(
                    "Gossip: Publishing {}: local_rate={:.2}",
                    scope_name,
                    fresh_rate
                );
                state.update_scope(scope_name.clone(), fresh_rate);
            }

            state
        };

        // 2. Publish to cluster (manager lock released)
        if let Err(e) = gossip.publish_state(&local_state).await {
            tracing::error!("Failed to publish gossip state: {}", e);
            continue;
        }

        // 3. Aggregate peer rates with staleness decay (manager lock released)
        let observations = gossip.get_peer_observations().await;
        let aggregated = aggregate_peer_rates(&observations, sync_interval, stale_timeout);

        // 4. Apply external rates, live peer count, and per-peer
        // observations in one pass. Scopes absent from the aggregate get an
        // explicit zero. The aggregated values are the transport's reporting
        // metrics; the control engine consumes the raw observations.
        let mut mgr = manager.write().await;
        for (scope_name, limiter) in mgr.limiters_mut() {
            let external_rate = aggregated
                .scope_rates
                .get(scope_name)
                .copied()
                .unwrap_or(0.0);
            limiter.set_external_accepted_request_rate(external_rate);
            limiter.set_num_peers(aggregated.live_peers);
            let obs: Vec<PeerRate<f64>> = observations
                .iter()
                .filter_map(|o| {
                    o.scope_rates.get(scope_name).map(|rate| PeerRate {
                        id: o.node_id.clone(),
                        rate: *rate,
                        age: o.age,
                    })
                })
                .collect();
            limiter.set_peer_observations(obs);
        }

        tracing::debug!(
            "Gossip sync: {} scopes, {} live peers, {} peer scopes",
            mgr.num_scopes(),
            aggregated.live_peers,
            aggregated.scope_rates.len()
        );
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
        // A limiter with a previously injected external rate must be reset to
        // zero by the sync loop when no live peers report that scope
        let mut mgr = RateLimitManager::new(100.0, 0.8, 0.05, 0.04);
        mgr.should_throttle("test-scope");
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
