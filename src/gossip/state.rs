//! Gossip state serialization for cluster coordination

use std::collections::HashMap;
use std::time::SystemTime;

#[cfg(feature = "server")]
use serde::{Deserialize, Serialize};

/// Node state shared via gossip protocol
#[cfg(feature = "server")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipState {
    /// Node identifier
    pub node_id: String,

    /// Per-scope rate information
    pub scopes: HashMap<String, ScopeState>,

    /// Timestamp of this state update
    pub timestamp: SystemTime,
}

/// Per-scope state for gossip
#[cfg(feature = "server")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeState {
    /// Local accepted request rate (requests/second)
    pub accepted_rate: f64,

    /// Timestamp of last update
    pub timestamp: SystemTime,
}

#[cfg(feature = "server")]
impl GossipState {
    /// Create new gossip state
    pub fn new(node_id: String) -> Self {
        GossipState {
            node_id,
            scopes: HashMap::new(),
            timestamp: SystemTime::now(),
        }
    }

    /// Add or update scope state
    pub fn update_scope(&mut self, scope: String, accepted_rate: f64) {
        self.scopes.insert(
            scope,
            ScopeState {
                accepted_rate,
                timestamp: SystemTime::now(),
            },
        );
        self.timestamp = SystemTime::now();
    }

    /// Serialize to JSON string for gossip
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize from JSON string
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    #[test]
    fn test_gossip_state_creation() {
        let state = GossipState::new("node-1".to_string());
        assert_eq!(state.node_id, "node-1");
        assert!(state.scopes.is_empty());
    }

    #[test]
    fn test_update_scope() {
        let mut state = GossipState::new("node-1".to_string());
        state.update_scope("test-scope".to_string(), 50.5);

        assert_eq!(state.scopes.len(), 1);
        assert_eq!(state.scopes.get("test-scope").unwrap().accepted_rate, 50.5);
    }

    #[test]
    fn test_json_roundtrip() {
        let mut state = GossipState::new("node-1".to_string());
        state.update_scope("scope-1".to_string(), 100.0);
        state.update_scope("scope-2".to_string(), 200.0);

        let json = state.to_json().expect("Failed to serialize");
        let deserialized = GossipState::from_json(&json).expect("Failed to deserialize");

        assert_eq!(deserialized.node_id, "node-1");
        assert_eq!(deserialized.scopes.len(), 2);
        assert_eq!(
            deserialized.scopes.get("scope-1").unwrap().accepted_rate,
            100.0
        );
        assert_eq!(
            deserialized.scopes.get("scope-2").unwrap().accepted_rate,
            200.0
        );
    }

    #[test]
    fn test_json_with_special_characters() {
        let mut state = GossipState::new("node-1".to_string());
        state.update_scope("api#premium_user@123".to_string(), 50.0);

        let json = state.to_json().expect("Failed to serialize");
        let deserialized = GossipState::from_json(&json).expect("Failed to deserialize");

        assert!(deserialized.scopes.contains_key("api#premium_user@123"));
    }
}
