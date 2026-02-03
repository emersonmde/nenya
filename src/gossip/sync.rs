//! Gossip synchronization loop for distributed rate limiting

use super::{GossipManager, GossipState};
use crate::api::RateLimitManager;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "server")]
use tokio::sync::RwLock;
#[cfg(feature = "server")]
use tokio::time::interval;

/// Run the gossip synchronization loop
///
/// This background task:
/// 1. Collects local scope rates every 1s
/// 2. Publishes state to cluster via gossip
/// 3. Aggregates peer rates
/// 4. Updates manager with external rates and peer count
#[cfg(feature = "server")]
pub async fn gossip_sync_loop(
    manager: Arc<RwLock<RateLimitManager>>,
    gossip: Arc<GossipManager>,
    node_id: String,
) {
    // 500ms interval for faster convergence and tighter synchronization
    let mut tick = interval(Duration::from_millis(500));

    loop {
        tick.tick().await;

        // 1. Collect local state with fresh rate calculations
        let local_state = {
            let mut mgr = manager.write().await;
            let mut state = GossipState::new(node_id.clone());

            let now = std::time::Instant::now();
            for (scope_name, _) in mgr.get_all_scopes() {
                // Update the limiter's state to get fresh rate measurement
                if let Some(limiter) = mgr.get_limiter_mut(&scope_name) {
                    limiter.update_state_at(now);
                    let fresh_rate = limiter.local_accepted_request_rate();
                    tracing::debug!(
                        "Gossip: Publishing {}: local_rate={:.2}",
                        scope_name,
                        fresh_rate
                    );
                    state.update_scope(scope_name, fresh_rate);
                }
            }

            state
        };

        // 2. Publish to cluster
        if let Err(e) = gossip.publish_state(&local_state).await {
            tracing::error!("Failed to publish gossip state: {}", e);
            continue;
        }

        // 3. Aggregate peer rates
        let peer_states = gossip.get_peer_states().await;
        let num_peers = peer_states.len();

        let mut aggregated: HashMap<String, f64> = HashMap::new();

        for peer_state in peer_states {
            tracing::debug!(
                "Gossip: Received state from node {} with {} scopes",
                peer_state.node_id,
                peer_state.scopes.len()
            );
            for (scope_name, scope_state) in peer_state.scopes {
                let prev_total = *aggregated.get(&scope_name).unwrap_or(&0.0);
                let new_total = prev_total + scope_state.accepted_rate;
                tracing::debug!(
                    "Gossip: {} from {}: rate={:.2}, aggregated: {:.2} -> {:.2}",
                    scope_name,
                    peer_state.node_id,
                    scope_state.accepted_rate,
                    prev_total,
                    new_total
                );
                *aggregated.entry(scope_name).or_default() += scope_state.accepted_rate;
            }
        }

        // 4. Update manager with external rates
        let aggregated_count = aggregated.len();
        let mut mgr = manager.write().await;

        for (scope_name, external_rate) in aggregated {
            if let Some(limiter) = mgr.get_limiter_mut(&scope_name) {
                tracing::debug!(
                    "Gossip: Setting external rate for {}: {:.2} (from {} peers)",
                    scope_name,
                    external_rate,
                    num_peers
                );
                limiter.set_external_accepted_request_rate(external_rate);
                limiter.set_num_peers(num_peers);
            }
        }

        // Also update peer count for scopes that don't have peers yet
        for (scope_name, _) in mgr.get_all_scopes() {
            if let Some(limiter) = mgr.get_limiter_mut(&scope_name) {
                limiter.set_num_peers(num_peers);
            }
        }

        tracing::debug!(
            "Gossip sync: {} scopes, {} peers, {} peer scopes",
            mgr.num_scopes(),
            num_peers,
            aggregated_count
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
            gossip_sync_loop(manager_clone, gossip_clone, "test-node".to_string()).await;
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
}
