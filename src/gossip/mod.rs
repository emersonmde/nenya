//! Gossip protocol for distributed coordination
//!
//! Chitchat integration and cluster state management

#[cfg(feature = "server")]
pub mod state;

#[cfg(feature = "server")]
pub mod manager;

#[cfg(feature = "server")]
pub mod sync;

#[cfg(feature = "server")]
pub use manager::GossipManager;

#[cfg(feature = "server")]
pub use state::{GossipState, ScopeState};

#[cfg(feature = "server")]
pub use sync::gossip_sync_loop;
