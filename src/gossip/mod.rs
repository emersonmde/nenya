//! Gossip protocol for distributed coordination
//!
//! Chitchat integration and cluster state management

#[cfg(feature = "server")]
pub mod aggregate;

#[cfg(feature = "server")]
pub mod state;

#[cfg(feature = "server")]
pub mod manager;

#[cfg(feature = "server")]
pub mod sync;

#[cfg(feature = "server")]
pub use aggregate::{aggregate_peer_rates, staleness_weight, AggregatedRates, PeerObservation};

#[cfg(feature = "server")]
pub use manager::GossipManager;

#[cfg(feature = "server")]
pub use state::{GossipState, ScopeState};

#[cfg(feature = "server")]
pub use sync::gossip_sync_loop;
