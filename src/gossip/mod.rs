//! Gossip protocol for distributed coordination
//!
//! Chitchat integration and cluster state management. The `aggregate`
//! submodule is transport-agnostic and is also compiled for the `sim`
//! feature so the deterministic simulator exercises the production
//! aggregation/decay logic.

pub mod aggregate;

#[cfg(feature = "server")]
pub mod state;

#[cfg(feature = "server")]
pub mod manager;

#[cfg(feature = "server")]
pub mod sync;

pub use aggregate::{aggregate_peer_rates, staleness_weight, AggregatedRates, PeerObservation};

#[cfg(feature = "server")]
pub use manager::GossipManager;

#[cfg(feature = "server")]
pub use state::{GossipState, ScopeState};

#[cfg(feature = "server")]
pub use sync::gossip_sync_loop;
