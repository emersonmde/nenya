pub mod assertions;
pub mod metrics_collector;
/// Test utilities for scenario and integration testing
///
/// This module provides reusable infrastructure for testing the PID rate limiter:
/// - Request generators: Various load patterns for testing
/// - Metrics collection: Time-series data capture
/// - Assertions: Statistical validation helpers
/// - Scenario runner: Simulated time loop execution
pub mod request_generators;
pub mod scenario_runner;

// Re-export commonly used types for convenience
pub use metrics_collector::*;
pub use request_generators::*;
pub use scenario_runner::*;
