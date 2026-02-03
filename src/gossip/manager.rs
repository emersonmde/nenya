//! Gossip manager wrapping Chitchat for cluster coordination

use super::state::GossipState;
use std::net::SocketAddr;
use std::time::Duration;

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

/// Manager for gossip protocol using Chitchat
#[cfg(feature = "server")]
pub struct GossipManager {
    /// Node identifier
    pub node_id: String,

    /// Chitchat handle for gossip operations
    handle: ChitchatHandle,

    /// Gossip listen address
    listen_addr: SocketAddr,
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

    /// Get states from all peer nodes (excluding self)
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

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

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
