//! Gossip protocol for distributed coordination
//!
//! Chitchat integration and cluster state management. The `aggregate` and
//! `tier` submodules are transport-agnostic and are also compiled for the
//! `sim` feature so the deterministic simulator exercises the production
//! aggregation/decay and two-tier promotion/demotion logic.

pub mod aggregate;
pub mod tier;

#[cfg(feature = "server")]
pub mod state;

#[cfg(feature = "server")]
pub mod manager;

#[cfg(feature = "server")]
pub mod sync;

pub use aggregate::{aggregate_peer_rates, staleness_weight, AggregatedRates, PeerObservation};
pub use tier::{budget_evictions, should_promote, DemotionTracker, TailScope, TierConfig};

#[cfg(feature = "server")]
pub use manager::GossipManager;

#[cfg(feature = "server")]
pub use state::{GossipState, ScopeState};

#[cfg(feature = "server")]
pub use sync::gossip_sync_loop;
