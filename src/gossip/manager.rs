//! Gossip manager wrapping Chitchat for cluster coordination
//!
//! # Wire format (Milestone 6)
//!
//! One chitchat key per gossiped scope — `s:<scope>` → rate as a compact
//! decimal — plus a per-node publish counter `nenya_v` used as an opaque
//! change marker for age tracking. Chitchat's anti-entropy versions each
//! key independently, so a sync round retransmits only the scopes whose
//! rates actually changed instead of one monolithic JSON blob (the
//! pre-Milestone-6 format re-shipped every scope on any change, ~115
//! bytes/scope of it serde-JSON `SystemTime` overhead). Values are rounded
//! to 3 decimals so idle scopes publish byte-identical values and cost
//! nothing per exchange. Scopes that leave the hot tier are deleted
//! (chitchat tombstones them and garbage-collects after the deletion grace
//! period).

use super::aggregate::PeerObservation;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Chitchat key prefix for per-scope accepted rates
const SCOPE_KEY_PREFIX: &str = "s:";

/// Chitchat key for the per-node publish counter (opaque change marker;
/// replaces the old wall-clock timestamp — it is only ever compared for
/// equality, never against any clock)
const VERSION_KEY: &str = "nenya_v";

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
    /// The peer's gossiped publish counter, used only as an opaque change
    /// marker (compared for equality, never interpreted)
    last_marker: String,

    /// Local monotonic time when that marker was first observed
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

    /// Publish counter (the `nenya_v` change marker)
    publish_counter: AtomicU64,

    /// Scope → last published value, for change suppression and deletion
    /// of scopes that left the hot tier
    published: Mutex<HashMap<String, String>>,
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
            publish_counter: AtomicU64::new(0),
            published: Mutex::new(HashMap::new()),
        })
    }

    /// Publish the hot-tier per-scope rates to the cluster: one chitchat
    /// key per scope (only re-set when the rounded value changed, so
    /// anti-entropy ships just the deltas), deletion for scopes that left
    /// the hot set, and a bumped `nenya_v` change marker.
    pub async fn publish_rates(&self, rates: &[(String, f64)]) -> Result<(), GossipError> {
        let counter = self.publish_counter.fetch_add(1, Ordering::Relaxed) + 1;

        // Diff against the previously published set outside the chitchat
        // callback (the mutex is uncontended: only the sync loop publishes)
        let (to_set, to_delete) = {
            let mut published = self.published.lock().expect("published mutex poisoned");
            let mut to_set: Vec<(String, String)> = Vec::new();
            for (scope, rate) in rates {
                let value = format!("{:.3}", rate);
                if published.get(scope) != Some(&value) {
                    published.insert(scope.clone(), value.clone());
                    to_set.push((format!("{}{}", SCOPE_KEY_PREFIX, scope), value));
                }
            }
            let current: std::collections::HashSet<&String> =
                rates.iter().map(|(scope, _)| scope).collect();
            let to_delete: Vec<String> = published
                .keys()
                .filter(|scope| !current.contains(scope))
                .map(|scope| format!("{}{}", SCOPE_KEY_PREFIX, scope))
                .collect();
            published.retain(|scope, _| current.contains(scope));
            (to_set, to_delete)
        };

        self.handle
            .with_chitchat(move |chitchat| {
                let state = chitchat.self_node_state();
                for (key, value) in &to_set {
                    state.set(key, value);
                }
                for key in &to_delete {
                    state.delete(key);
                }
                state.set(VERSION_KEY, counter.to_string());
            })
            .await;

        Ok(())
    }

    /// Get per-peer observations with locally measured ages, for age-weighted
    /// aggregation.
    ///
    /// Age is time since the peer's gossiped `nenya_v` counter was last seen
    /// to *change*, measured on the local monotonic clock (age-at-receipt).
    /// The counter is an opaque change marker compared only for equality —
    /// no cross-node clock comparison exists anywhere, so clock skew cannot
    /// produce negative ages or amplified rates. A healthy peer republishes
    /// with a bumped counter every sync interval, keeping its age near zero;
    /// a crashed or partitioned peer's counter freezes and its age grows
    /// until decay drops it.
    ///
    /// Only nodes Chitchat considers live are included, and the local node
    /// is always skipped — peer rates can never double-count local traffic.
    /// Nodes with zero hot scopes still publish the counter, so membership
    /// liveness flows regardless of gossip payload.
    pub async fn get_peer_observations(&self) -> Vec<PeerObservation> {
        // (node_id, change marker, per-scope rates) per live peer
        let raw: Vec<(String, String, HashMap<String, f64>)> = self
            .handle
            .with_chitchat(|chitchat| {
                let self_id = chitchat.self_chitchat_id().clone();
                let mut raw = Vec::new();
                for chitchat_id in chitchat.live_nodes() {
                    if chitchat_id == &self_id {
                        continue;
                    }
                    let Some(node_state) = chitchat.node_state(chitchat_id) else {
                        continue;
                    };
                    // A node that never published is not an observation
                    let Some(marker) = node_state.get(VERSION_KEY) else {
                        continue;
                    };
                    let scope_rates: HashMap<String, f64> = node_state
                        .iter_prefix(SCOPE_KEY_PREFIX)
                        .filter_map(|(key, versioned)| {
                            let scope = &key[SCOPE_KEY_PREFIX.len()..];
                            versioned
                                .value
                                .parse::<f64>()
                                .ok()
                                .map(|rate| (scope.to_string(), rate))
                        })
                        .collect();
                    raw.push((chitchat_id.node_id.clone(), marker.to_string(), scope_rates));
                }
                raw
            })
            .await;

        let now = Instant::now();
        let mut receipts = self.receipts.lock().expect("receipts mutex poisoned");

        // Drop receipts for peers Chitchat has evicted (phi accrual failure
        // detection removes them from the live set) so the map can't grow
        // unboundedly with node churn
        receipts.retain(|node_id, _| raw.iter().any(|(id, _, _)| id == node_id));

        raw.into_iter()
            .map(|(node_id, marker, scope_rates)| {
                let age = observation_age(&mut receipts, &node_id, &marker, now);
                PeerObservation {
                    node_id,
                    age,
                    scope_rates,
                }
            })
            .collect()
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
/// If the peer's gossiped change marker differs from the last one seen (or
/// the peer is new), the receipt resets and the age is zero; otherwise the
/// age is the local monotonic time elapsed since that marker was first
/// observed. The marker is compared only for equality — its actual value is
/// irrelevant.
#[cfg(feature = "server")]
fn observation_age(
    receipts: &mut HashMap<String, PeerReceipt>,
    node_id: &str,
    marker: &str,
    now: Instant,
) -> Duration {
    match receipts.get_mut(node_id) {
        Some(receipt) if receipt.last_marker == marker => {
            now.saturating_duration_since(receipt.received_at)
        }
        _ => {
            receipts.insert(
                node_id.to_string(),
                PeerReceipt {
                    last_marker: marker.to_string(),
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

        let age = observation_age(&mut receipts, "peer-1", "1", now);
        assert_eq!(age, Duration::ZERO);
    }

    #[test]
    fn test_observation_age_grows_while_marker_frozen() {
        let mut receipts = HashMap::new();
        let start = Instant::now();

        observation_age(&mut receipts, "peer-1", "7", start);

        // Same marker seen 3 seconds later (local clock): peer went silent
        let age = observation_age(&mut receipts, "peer-1", "7", start + Duration::from_secs(3));
        assert_eq!(age, Duration::from_secs(3));
    }

    #[test]
    fn test_observation_age_resets_on_new_marker() {
        let mut receipts = HashMap::new();
        let start = Instant::now();

        observation_age(&mut receipts, "peer-1", "7", start);

        // Peer republished with a bumped counter: age resets to zero
        let age = observation_age(&mut receipts, "peer-1", "8", start + Duration::from_secs(3));
        assert_eq!(age, Duration::ZERO);

        // And grows again from there while frozen
        let age = observation_age(&mut receipts, "peer-1", "8", start + Duration::from_secs(5));
        assert_eq!(age, Duration::from_secs(2));
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

        let result = manager
            .publish_rates(&[("test-scope".to_string(), 100.0)])
            .await;
        assert!(result.is_ok());

        // Verify we can retrieve peer observations (empty: we're alone)
        let peers = manager.get_peer_observations().await;
        assert_eq!(peers.len(), 0);

        let num_peers = manager.num_peers().await;
        assert_eq!(num_peers, 0);

        // Clean shutdown
        let _ = manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_publish_diffs_and_deletes_scope_keys() {
        let listen_addr = "127.0.0.1:10002".parse().unwrap();
        let manager = GossipManager::new(
            "test-node".to_string(),
            listen_addr,
            vec![],
            "test-cluster".to_string(),
        )
        .await
        .expect("Failed to create manager");

        manager
            .publish_rates(&[("a".to_string(), 10.0), ("b".to_string(), 20.0)])
            .await
            .unwrap();
        // Scope `b` leaves the hot set: its key must be deleted
        manager
            .publish_rates(&[("a".to_string(), 10.0)])
            .await
            .unwrap();

        let (has_a, has_b, marker) = manager
            .handle
            .with_chitchat(|chitchat| {
                let state = chitchat.self_node_state();
                (
                    state.get("s:a").map(|v| v.to_string()),
                    state.get("s:b").map(|v| v.to_string()),
                    state.get(VERSION_KEY).map(|v| v.to_string()),
                )
            })
            .await;
        assert_eq!(has_a.as_deref(), Some("10.000"));
        assert_eq!(has_b, None, "removed scope key must be deleted");
        assert_eq!(marker.as_deref(), Some("2"), "counter bumps every publish");

        let _ = manager.shutdown().await;
    }
}
