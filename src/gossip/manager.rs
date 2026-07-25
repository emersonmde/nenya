//! Gossip manager wrapping Chitchat for cluster coordination

use super::aggregate::PeerObservation;
use super::state::GossipState;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

#[cfg(feature = "server")]
use chitchat::{spawn_chitchat, ChitchatConfig, ChitchatHandle, ChitchatId, FailureDetectorConfig};

#[cfg(feature = "server")]
use chitchat::transport::UdpTransport;

/// Errors that can occur during gossip operations
#[cfg(feature = "server")]
#[derive(Debug)]
pub enum GossipError {
    /// Failed to initialize Chitchat
    InitializationError(String),

    /// Failed to publish state
    PublishError(String),

    /// Failed to retrieve peer state
    RetrievalError(String),
}

#[cfg(feature = "server")]
impl std::fmt::Display for GossipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GossipError::InitializationError(msg) => {
                write!(f, "Gossip initialization error: {}", msg)
            }
            GossipError::PublishError(msg) => write!(f, "Gossip publish error: {}", msg),
            GossipError::RetrievalError(msg) => write!(f, "Gossip retrieval error: {}", msg),
        }
    }
}

#[cfg(feature = "server")]
impl std::error::Error for GossipError {}

/// Record of when a peer's state was last observed to change, on the local
/// monotonic clock
#[cfg(feature = "server")]
struct PeerReceipt {
    /// The peer's gossiped timestamp, used only as an opaque change marker
    /// (compared for equality, never against local time)
    last_timestamp: SystemTime,

    /// Local monotonic time when that timestamp was first observed
    received_at: Instant,
}

/// Manager for gossip protocol using Chitchat
#[cfg(feature = "server")]
pub struct GossipManager {
    /// Node identifier
    pub node_id: String,

    /// Chitchat handle for gossip operations
    handle: ChitchatHandle,

    /// Gossip listen address
    listen_addr: SocketAddr,

    /// Age-at-receipt tracking per peer, for staleness decay
    receipts: Mutex<HashMap<String, PeerReceipt>>,
}

#[cfg(feature = "server")]
impl GossipManager {
    /// Create a new gossip manager
    pub async fn new(
        node_id: String,
        listen_addr: SocketAddr,
        seed_nodes: Vec<String>,
        cluster_id: String,
    ) -> Result<Self, GossipError> {
        // Create ChitchatId
        let chitchat_id = ChitchatId::new(node_id.clone(), 0, listen_addr);

        // Create Chitchat configuration
        let chitchat_config = ChitchatConfig {
            chitchat_id,
            cluster_id,
            gossip_interval: Duration::from_secs(1),
            listen_addr,
            seed_nodes,
            failure_detector_config: FailureDetectorConfig::default(),
            marked_for_deletion_grace_period: Duration::from_secs(60),
            catchup_callback: None,
            extra_liveness_predicate: None,
        };

        // Create UDP transport
        let transport = UdpTransport;

        // Spawn Chitchat server
        let handle = spawn_chitchat(chitchat_config, vec![], &transport)
            .await
            .map_err(|e| GossipError::InitializationError(format!("{:?}", e)))?;

        Ok(GossipManager {
            node_id,
            handle,
            listen_addr,
            receipts: Mutex::new(HashMap::new()),
        })
    }

    /// Publish local state to the cluster
    pub async fn publish_state(&self, state: &GossipState) -> Result<(), GossipError> {
        let json = state
            .to_json()
            .map_err(|e| GossipError::PublishError(format!("JSON serialization error: {}", e)))?;

        // Update Chitchat state with serialized JSON
        self.handle
            .with_chitchat(|chitchat| {
                chitchat.self_node_state().set("nenya_state", json.clone());
            })
            .await;

        Ok(())
    }

    /// Get per-peer observations with locally measured ages, for age-weighted
    /// aggregation.
    ///
    /// Age is time since the peer's gossiped timestamp was last seen to
    /// *change*, measured on the local monotonic clock (age-at-receipt). The
    /// peer's `SystemTime` is used only as an opaque change marker and is never
    /// compared against local time, so cross-node clock skew — including
    /// future-dated timestamps — cannot produce negative ages or amplified
    /// rates. A healthy peer republishes with a fresh timestamp every sync
    /// interval, keeping its age near zero; a crashed or partitioned peer's
    /// timestamp freezes and its age grows until decay drops it.
    pub async fn get_peer_observations(&self) -> Vec<PeerObservation> {
        let states = self.get_peer_states().await;
        let now = Instant::now();

        let mut receipts = self.receipts.lock().expect("receipts mutex poisoned");

        // Drop receipts for peers Chitchat has evicted (phi accrual failure
        // detection removes them from the live set) so the map can't grow
        // unboundedly with node churn
        receipts.retain(|node_id, _| states.iter().any(|s| &s.node_id == node_id));

        states
            .into_iter()
            .map(|state| {
                let age = observation_age(&mut receipts, &state.node_id, state.timestamp, now);
                PeerObservation {
                    node_id: state.node_id,
                    age,
                    scope_rates: state
                        .scopes
                        .into_iter()
                        .map(|(scope, s)| (scope, s.accepted_rate))
                        .collect(),
                }
            })
            .collect()
    }

    /// Get states from all peer nodes (excluding self)
    ///
    /// Only nodes Chitchat considers live are included, and the local node is
    /// always skipped — peer rates can never double-count local traffic.
    pub async fn get_peer_states(&self) -> Vec<GossipState> {
        self.handle
            .with_chitchat(|chitchat| {
                let mut states = Vec::new();
                let self_id = chitchat.self_chitchat_id();

                // Iterate through all live nodes
                for chitchat_id in chitchat.live_nodes() {
                    // Skip self
                    if chitchat_id == self_id {
                        continue;
                    }

                    // Try to get node state
                    if let Some(node_state) = chitchat.node_state(chitchat_id) {
                        // Try to get nenya state
                        if let Some(json) = node_state.get("nenya_state") {
                            if let Ok(state) = GossipState::from_json(json) {
                                states.push(state);
                            }
                        }
                    }
                }

                states
            })
            .await
    }

    /// Get the number of alive peers (excluding self)
    pub async fn num_peers(&self) -> usize {
        self.handle
            .with_chitchat(|chitchat| {
                let total_nodes = chitchat.live_nodes().count();

                // Subtract 1 for self
                if total_nodes > 0 {
                    total_nodes - 1
                } else {
                    0
                }
            })
            .await
    }

    /// Get listen address
    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    /// Shutdown the gossip manager
    pub async fn shutdown(self) -> Result<(), GossipError> {
        self.handle
            .shutdown()
            .await
            .map_err(|e| GossipError::InitializationError(format!("Shutdown error: {:?}", e)))
    }
}

/// Compute a peer observation's age from receipt records.
///
/// If the peer's gossiped `timestamp` differs from the last one seen (or the
/// peer is new), the receipt resets and the age is zero; otherwise the age is
/// the local monotonic time elapsed since that timestamp was first observed.
/// The timestamp is compared only for equality — its actual value (past,
/// future, skewed) is irrelevant.
#[cfg(feature = "server")]
fn observation_age(
    receipts: &mut HashMap<String, PeerReceipt>,
    node_id: &str,
    timestamp: SystemTime,
    now: Instant,
) -> Duration {
    match receipts.get_mut(node_id) {
        Some(receipt) if receipt.last_timestamp == timestamp => {
            now.saturating_duration_since(receipt.received_at)
        }
        _ => {
            receipts.insert(
                node_id.to_string(),
                PeerReceipt {
                    last_timestamp: timestamp,
                    received_at: now,
                },
            );
            Duration::ZERO
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    #[test]
    fn test_observation_age_new_peer_is_zero() {
        let mut receipts = HashMap::new();
        let now = Instant::now();
        let ts = SystemTime::now();

        let age = observation_age(&mut receipts, "peer-1", ts, now);
        assert_eq!(age, Duration::ZERO);
    }

    #[test]
    fn test_observation_age_grows_while_timestamp_frozen() {
        let mut receipts = HashMap::new();
        let start = Instant::now();
        let ts = SystemTime::now();

        observation_age(&mut receipts, "peer-1", ts, start);

        // Same timestamp seen 3 seconds later (local clock): peer went silent
        let age = observation_age(&mut receipts, "peer-1", ts, start + Duration::from_secs(3));
        assert_eq!(age, Duration::from_secs(3));
    }

    #[test]
    fn test_observation_age_resets_on_new_timestamp() {
        let mut receipts = HashMap::new();
        let start = Instant::now();
        let ts1 = SystemTime::now();

        observation_age(&mut receipts, "peer-1", ts1, start);

        // Peer republished with a new timestamp: age resets to zero
        let ts2 = ts1 + Duration::from_millis(500);
        let age = observation_age(&mut receipts, "peer-1", ts2, start + Duration::from_secs(3));
        assert_eq!(age, Duration::ZERO);

        // And grows again from there while frozen
        let age = observation_age(&mut receipts, "peer-1", ts2, start + Duration::from_secs(5));
        assert_eq!(age, Duration::from_secs(2));
    }

    #[test]
    fn test_observation_age_future_dated_timestamp_harmless() {
        let mut receipts = HashMap::new();
        let start = Instant::now();

        // Peer's clock is an hour ahead: timestamp is only a change marker,
        // so the age is still measured on the local clock
        let future_ts = SystemTime::now() + Duration::from_secs(3600);
        let age = observation_age(&mut receipts, "peer-1", future_ts, start);
        assert_eq!(age, Duration::ZERO);

        let age = observation_age(
            &mut receipts,
            "peer-1",
            future_ts,
            start + Duration::from_secs(2),
        );
        assert_eq!(age, Duration::from_secs(2));
    }

    #[tokio::test]
    async fn test_gossip_state_roundtrip() {
        let mut state = GossipState::new("test-node".to_string());
        state.update_scope("test-scope".to_string(), 50.0);

        let json = state.to_json().expect("Failed to serialize");
        let deserialized = GossipState::from_json(&json).expect("Failed to deserialize");

        assert_eq!(deserialized.node_id, "test-node");
        assert_eq!(deserialized.scopes.len(), 1);
    }

    #[tokio::test]
    async fn test_gossip_manager_creation() {
        // Use a unique port for this test
        let listen_addr = "127.0.0.1:10000".parse().unwrap();

        let result = GossipManager::new(
            "test-node".to_string(),
            listen_addr,
            vec![],
            "test-cluster".to_string(),
        )
        .await;

        assert!(result.is_ok());

        let manager = result.unwrap();
        assert_eq!(manager.node_id, "test-node");
        assert_eq!(manager.listen_addr, listen_addr);

        // Clean shutdown
        let _ = manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_publish_and_retrieve_state() {
        let listen_addr = "127.0.0.1:10001".parse().unwrap();

        let manager = GossipManager::new(
            "test-node".to_string(),
            listen_addr,
            vec![],
            "test-cluster".to_string(),
        )
        .await
        .expect("Failed to create manager");

        // Create and publish state
        let mut state = GossipState::new("test-node".to_string());
        state.update_scope("test-scope".to_string(), 100.0);

        let result = manager.publish_state(&state).await;
        assert!(result.is_ok());

        // Verify we can retrieve peer states (should be empty since we're alone)
        let peers = manager.get_peer_states().await;
        assert_eq!(peers.len(), 0);

        let num_peers = manager.num_peers().await;
        assert_eq!(num_peers, 0);

        // Clean shutdown
        let _ = manager.shutdown().await;
    }
}
