//! Deterministic multi-node simulator (Milestone 4).
//!
//! The simulator is the project's primary tool for correctness testing,
//! benchmarking, and control-loop tuning: N in-process nodes with real
//! `RateLimiter`s, a message-bus gossip model (delay, jitter, loss,
//! partitions), seeded workloads, and a virtual clock — no wall-clock
//! sleeps, no network, no threads. A 60-second scenario runs in
//! milliseconds, and the same seed always produces byte-identical results.
//!
//! Aggregation/staleness-decay behavior comes from [`crate::gossip::aggregate`]
//! — the production code, not a reimplementation — so simulator findings
//! about convergence, overshoot, and partition behavior transfer to real
//! clusters (modulo the simplified transport model).
//!
//! # Example
//!
//! ```rust
//! use nenya::sim::scenario;
//!
//! let result = scenario::steady_above()
//!     .duration(std::time::Duration::from_secs(20))
//!     .run(42);
//! assert_eq!(result.samples.len(), 40);
//! // Same seed, same result — byte for byte
//! let again = scenario::steady_above()
//!     .duration(std::time::Duration::from_secs(20))
//!     .run(42);
//! assert_eq!(result.to_csv(), again.to_csv());
//! ```

pub mod cluster;
pub mod metrics;
pub mod rng;
pub mod scenario;
pub mod workload;

pub use cluster::{GossipModel, SimCluster, SimConfig, SimEvent, TickCounts};
pub use metrics::{Convergence, RunResult, Sample, Summary};
pub use rng::SplitMix64;
pub use scenario::Scenario;
pub use workload::{ArrivalProcess, LoadPattern, SineComponent, Workload};
