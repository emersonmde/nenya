//! Configuration management
//!
//! Handles environment variable configuration for nenya-sentinel server.
//!
//! # Environment Variables
//!
//! - `NENYA_CLUSTER_SECRET`: **Required** - Secret for cluster authentication
//! - `NENYA_LISTEN_ADDR`: HTTP API listen address (default: 127.0.0.1:8080)
//! - `NENYA_GOSSIP_ADDR`: Gossip protocol listen address (default: 0.0.0.0:8081)
//! - `NENYA_SEED_NODES`: Comma-separated list of seed nodes (host:port)
//! - `NENYA_DEFAULT_TARGET_RATE`: Default target rate for new scopes (default: 100.0)
//! - `NENYA_DEFAULT_CLUSTER_TARGET`: Cluster-wide target rate when distributed (default: 300.0)
//! - `NENYA_NODE_ID`: Optional node identifier (default: hostname)
//! - `NENYA_SYNC_INTERVAL_MS`: Gossip sync loop interval in ms (default: 500)
//! - `NENYA_STALE_TIMEOUT_MS`: Age at which a silent peer's rate contribution
//!   decays to zero and it stops counting as live (default: 10000)
//! - `NENYA_DEFAULT_ENGINE`: Control engine: `pid` | `bayesian` | `hybrid`
//!   (default: pid). Always explicit — never selected at runtime.
//! - `NENYA_BAYESIAN_PROCESS_NOISE`: Estimator process noise `q` (rps²/s)
//! - `NENYA_BAYESIAN_MEASUREMENT_NOISE`: Estimator measurement noise `r` (rps²)
//! - `NENYA_BAYESIAN_CONFIDENCE_Z`: Admission confidence multiplier `z`
//! - `NENYA_PROMOTE_UTILIZATION`: Two-tier promotion threshold (fraction of
//!   estimated cluster utilization; default: sweep-derived, see
//!   `gossip::tier`)
//! - `NENYA_DEMOTE_UTILIZATION`: Two-tier demotion threshold (must be below
//!   the promotion threshold)
//! - `NENYA_DEMOTE_HOLD_SECS`: Demotion hysteresis hold in seconds
//! - `NENYA_GOSSIP_BUDGET`: Per-node hard cap on gossiped (hot-tier) scopes

use std::env;
use std::net::SocketAddr;
use std::time::Duration;

#[cfg(feature = "server")]
use serde::{Deserialize, Serialize};

/// Configuration errors
#[derive(Debug)]
pub enum ConfigError {
    /// Missing required environment variable
    MissingRequired(String),
    /// Invalid value for environment variable
    InvalidValue(String, String),
    /// Parse error
    ParseError(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::MissingRequired(name) => {
                write!(f, "Missing required environment variable: {}", name)
            }
            ConfigError::InvalidValue(name, msg) => {
                write!(f, "Invalid value for {}: {}", name, msg)
            }
            ConfigError::ParseError(msg) => write!(f, "Parse error: {}", msg),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Server configuration
#[cfg(feature = "server")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Cluster secret for authentication
    pub cluster_secret: String,

    /// HTTP API listen address
    pub listen_addr: SocketAddr,

    /// Gossip protocol listen address
    pub gossip_addr: SocketAddr,

    /// Seed nodes for cluster bootstrap
    pub seed_nodes: Vec<SocketAddr>,

    /// Node identifier (defaults to hostname)
    pub node_id: String,

    /// Default target rate for new scopes (single-node mode)
    pub default_target_rate: f64,

    /// Default cluster-wide target rate (distributed mode)
    pub default_cluster_target: f64,

    /// Minimum rate bound for PID controller
    pub default_min_rate: f64,

    /// Maximum rate bound for PID controller
    pub default_max_rate: f64,

    /// PID proportional gain
    pub default_kp: f64,

    /// PID integral gain
    pub default_ki: f64,

    /// PID derivative gain
    pub default_kd: f64,

    /// Gossip sync loop interval
    pub sync_interval: Duration,

    /// Age past which a silent peer's gossiped rate is fully discounted
    pub stale_timeout: Duration,

    /// Control engine for the default scope pattern
    pub default_engine: crate::engine::EngineKind,

    /// Estimator process noise `q` override (bayesian/hybrid engines)
    pub bayesian_process_noise: Option<f64>,

    /// Estimator measurement noise `r` override (bayesian/hybrid engines)
    pub bayesian_measurement_noise: Option<f64>,

    /// Admission confidence multiplier `z` override (bayesian engine)
    pub bayesian_confidence_z: Option<f64>,

    /// Two-tier promotion threshold override (fraction of estimated
    /// cluster utilization; `None` = sweep-derived default)
    pub promote_utilization: Option<f64>,

    /// Two-tier demotion threshold override (must stay below promotion)
    pub demote_utilization: Option<f64>,

    /// Demotion hysteresis hold override in seconds
    pub demote_hold_secs: Option<f64>,

    /// Per-node hard cap on gossiped (hot-tier) scopes
    pub gossip_budget: usize,
}

#[cfg(feature = "server")]
impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self, ConfigError> {
        let cluster_secret = env::var("NENYA_CLUSTER_SECRET")
            .map_err(|_| ConfigError::MissingRequired("NENYA_CLUSTER_SECRET".to_string()))?;

        if cluster_secret.is_empty() {
            return Err(ConfigError::InvalidValue(
                "NENYA_CLUSTER_SECRET".to_string(),
                "cannot be empty".to_string(),
            ));
        }

        let listen_addr = env::var("NENYA_LISTEN_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
            .parse()
            .map_err(|e| {
                ConfigError::InvalidValue(
                    "NENYA_LISTEN_ADDR".to_string(),
                    format!("invalid socket address: {}", e),
                )
            })?;

        let gossip_addr = env::var("NENYA_GOSSIP_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:8081".to_string())
            .parse()
            .map_err(|e| {
                ConfigError::InvalidValue(
                    "NENYA_GOSSIP_ADDR".to_string(),
                    format!("invalid socket address: {}", e),
                )
            })?;

        let seed_nodes = env::var("NENYA_SEED_NODES")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                s.trim().parse().map_err(|e| {
                    ConfigError::InvalidValue(
                        "NENYA_SEED_NODES".to_string(),
                        format!("invalid seed node '{}': {}", s, e),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let node_id = env::var("NENYA_NODE_ID").unwrap_or_else(|_| {
            hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| format!("node-{}", std::process::id()))
        });

        let default_target_rate = env::var("NENYA_DEFAULT_TARGET_RATE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100.0);

        if default_target_rate <= 0.0 {
            return Err(ConfigError::InvalidValue(
                "NENYA_DEFAULT_TARGET_RATE".to_string(),
                "must be positive".to_string(),
            ));
        }

        let default_cluster_target = env::var("NENYA_DEFAULT_CLUSTER_TARGET")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300.0);

        if default_cluster_target <= 0.0 {
            return Err(ConfigError::InvalidValue(
                "NENYA_DEFAULT_CLUSTER_TARGET".to_string(),
                "must be positive".to_string(),
            ));
        }
        let default_min_rate = env::var("NENYA_DEFAULT_MIN_RATE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default_target_rate * 0.5);

        let default_max_rate = env::var("NENYA_DEFAULT_MAX_RATE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default_target_rate * 2.0);

        let default_kp = env::var("NENYA_DEFAULT_KP")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.5);

        let default_ki = env::var("NENYA_DEFAULT_KI")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.02);

        let default_kd = env::var("NENYA_DEFAULT_KD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.08);

        let sync_interval_ms: u64 = env::var("NENYA_SYNC_INTERVAL_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(500);

        if sync_interval_ms == 0 {
            return Err(ConfigError::InvalidValue(
                "NENYA_SYNC_INTERVAL_MS".to_string(),
                "must be positive".to_string(),
            ));
        }

        let stale_timeout_ms: u64 = env::var("NENYA_STALE_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10_000);

        // The full-weight window is 2 × sync_interval; a stale timeout inside
        // it leaves no decay span (hard cutoff) and is almost certainly a
        // misconfiguration
        if stale_timeout_ms <= 2 * sync_interval_ms {
            return Err(ConfigError::InvalidValue(
                "NENYA_STALE_TIMEOUT_MS".to_string(),
                format!(
                    "must exceed 2 × sync interval ({} ms)",
                    2 * sync_interval_ms
                ),
            ));
        }

        let default_engine = match env::var("NENYA_DEFAULT_ENGINE") {
            Ok(s) => s.parse().map_err(|e: String| {
                ConfigError::InvalidValue("NENYA_DEFAULT_ENGINE".to_string(), e)
            })?,
            Err(_) => crate::engine::EngineKind::Pid,
        };

        let parse_positive = |name: &str| -> Result<Option<f64>, ConfigError> {
            match env::var(name) {
                Ok(s) => {
                    let v: f64 = s.parse().map_err(|_| {
                        ConfigError::InvalidValue(name.to_string(), "not a number".to_string())
                    })?;
                    if v <= 0.0 {
                        return Err(ConfigError::InvalidValue(
                            name.to_string(),
                            "must be positive".to_string(),
                        ));
                    }
                    Ok(Some(v))
                }
                Err(_) => Ok(None),
            }
        };
        let bayesian_process_noise = parse_positive("NENYA_BAYESIAN_PROCESS_NOISE")?;
        let bayesian_measurement_noise = parse_positive("NENYA_BAYESIAN_MEASUREMENT_NOISE")?;
        // z = 0 (admit against the raw mean) is legitimate
        let bayesian_confidence_z = match env::var("NENYA_BAYESIAN_CONFIDENCE_Z") {
            Ok(s) => Some(s.parse::<f64>().map_err(|_| {
                ConfigError::InvalidValue(
                    "NENYA_BAYESIAN_CONFIDENCE_Z".to_string(),
                    "not a number".to_string(),
                )
            })?),
            Err(_) => None,
        };

        let promote_utilization = parse_positive("NENYA_PROMOTE_UTILIZATION")?;
        let demote_utilization = parse_positive("NENYA_DEMOTE_UTILIZATION")?;
        let demote_hold_secs = parse_positive("NENYA_DEMOTE_HOLD_SECS")?;

        let gossip_budget: usize = match env::var("NENYA_GOSSIP_BUDGET") {
            Ok(s) => s.parse().map_err(|_| {
                ConfigError::InvalidValue(
                    "NENYA_GOSSIP_BUDGET".to_string(),
                    "not a positive integer".to_string(),
                )
            })?,
            Err(_) => crate::gossip::tier::DEFAULT_GOSSIP_BUDGET,
        };

        // Validate the combined tier policy (thresholds ordered, budget ≥ 1)
        {
            let defaults = crate::gossip::tier::TierConfig::default();
            let tier = crate::gossip::tier::TierConfig {
                promote_utilization: promote_utilization.unwrap_or(defaults.promote_utilization),
                demote_utilization: demote_utilization.unwrap_or(defaults.demote_utilization),
                demote_hold: demote_hold_secs
                    .map(Duration::from_secs_f64)
                    .unwrap_or(defaults.demote_hold),
                gossip_budget,
            };
            tier.validate().map_err(|e| {
                ConfigError::InvalidValue("NENYA_PROMOTE/DEMOTE_UTILIZATION".to_string(), e)
            })?;
        }

        Ok(Config {
            cluster_secret,
            listen_addr,
            gossip_addr,
            seed_nodes,
            node_id,
            default_target_rate,
            default_cluster_target,
            default_min_rate,
            default_max_rate,
            default_kp,
            default_ki,
            default_kd,
            sync_interval: Duration::from_millis(sync_interval_ms),
            stale_timeout: Duration::from_millis(stale_timeout_ms),
            default_engine,
            bayesian_process_noise,
            bayesian_measurement_noise,
            bayesian_confidence_z,
            promote_utilization,
            demote_utilization,
            demote_hold_secs,
            gossip_budget,
        })
    }

    /// Create a test configuration with sensible defaults
    #[cfg(test)]
    pub fn test_config() -> Self {
        Config {
            cluster_secret: "test-secret".to_string(),
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            gossip_addr: "127.0.0.1:8081".parse().unwrap(),
            seed_nodes: vec![],
            node_id: "test-node".to_string(),
            default_target_rate: 100.0,
            default_cluster_target: 300.0,
            default_min_rate: 50.0,
            default_max_rate: 200.0,
            // AIMD-inspired tuning for distributed systems:
            // - Lower Kp (0.5) for conservative response (additive increase)
            // - Lower Ki (0.02) to prevent integral windup from gossip lag
            // - Higher Kd (0.08) for stronger damping to reduce oscillation
            default_kp: 0.5,
            default_ki: 0.02,
            default_kd: 0.08,
            sync_interval: Duration::from_millis(500),
            stale_timeout: Duration::from_secs(10),
            default_engine: crate::engine::EngineKind::Pid,
            bayesian_process_noise: None,
            bayesian_measurement_noise: None,
            bayesian_confidence_z: None,
            promote_utilization: None,
            demote_utilization: None,
            demote_hold_secs: None,
            gossip_budget: crate::gossip::tier::DEFAULT_GOSSIP_BUDGET,
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;

    fn clean_env() {
        env::remove_var("NENYA_CLUSTER_SECRET");
        env::remove_var("NENYA_LISTEN_ADDR");
        env::remove_var("NENYA_GOSSIP_ADDR");
        env::remove_var("NENYA_SEED_NODES");
        env::remove_var("NENYA_NODE_ID");
        env::remove_var("NENYA_DEFAULT_TARGET_RATE");
        env::remove_var("NENYA_DEFAULT_CLUSTER_TARGET");
        env::remove_var("NENYA_DEFAULT_MIN_RATE");
        env::remove_var("NENYA_DEFAULT_MAX_RATE");
        env::remove_var("NENYA_DEFAULT_KP");
        env::remove_var("NENYA_DEFAULT_KI");
        env::remove_var("NENYA_DEFAULT_KD");
        env::remove_var("NENYA_SYNC_INTERVAL_MS");
        env::remove_var("NENYA_STALE_TIMEOUT_MS");
        env::remove_var("NENYA_DEFAULT_ENGINE");
        env::remove_var("NENYA_BAYESIAN_PROCESS_NOISE");
        env::remove_var("NENYA_BAYESIAN_MEASUREMENT_NOISE");
        env::remove_var("NENYA_BAYESIAN_CONFIDENCE_Z");
        env::remove_var("NENYA_PROMOTE_UTILIZATION");
        env::remove_var("NENYA_DEMOTE_UTILIZATION");
        env::remove_var("NENYA_DEMOTE_HOLD_SECS");
        env::remove_var("NENYA_GOSSIP_BUDGET");
    }

    #[test]
    #[serial]
    fn test_tier_thresholds_validated() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        // Demotion at/above promotion is a misconfiguration
        env::set_var("NENYA_PROMOTE_UTILIZATION", "0.4");
        env::set_var("NENYA_DEMOTE_UTILIZATION", "0.4");
        assert!(Config::from_env().is_err());

        env::set_var("NENYA_DEMOTE_UTILIZATION", "0.2");
        let config = Config::from_env().unwrap();
        assert_eq!(config.promote_utilization, Some(0.4));
        assert_eq!(config.demote_utilization, Some(0.2));
        assert_eq!(
            config.gossip_budget,
            crate::gossip::tier::DEFAULT_GOSSIP_BUDGET
        );
        clean_env();
    }

    #[test]
    #[serial]
    fn test_engine_selection() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        assert_eq!(
            Config::from_env().unwrap().default_engine,
            crate::engine::EngineKind::Pid
        );

        env::set_var("NENYA_DEFAULT_ENGINE", "bayesian");
        env::set_var("NENYA_BAYESIAN_PROCESS_NOISE", "5.0");
        let config = Config::from_env().unwrap();
        assert_eq!(config.default_engine, crate::engine::EngineKind::Bayesian);
        assert_eq!(config.bayesian_process_noise, Some(5.0));

        env::set_var("NENYA_DEFAULT_ENGINE", "nonsense");
        assert!(Config::from_env().is_err());
        clean_env();
    }

    #[test]
    #[serial]
    fn test_missing_cluster_secret() {
        clean_env();
        let result = Config::from_env();
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::MissingRequired(name) => {
                assert_eq!(name, "NENYA_CLUSTER_SECRET");
            }
            _ => panic!("Expected MissingRequired error"),
        }
    }

    #[test]
    #[serial]
    fn test_empty_cluster_secret() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "");
        let result = Config::from_env();
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::InvalidValue(name, _) => {
                assert_eq!(name, "NENYA_CLUSTER_SECRET");
            }
            _ => panic!("Expected InvalidValue error"),
        }
    }

    #[test]
    #[serial]
    fn test_minimal_config() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test-secret-123");

        let config = Config::from_env().expect("Failed to load config");

        assert_eq!(config.cluster_secret, "test-secret-123");
        assert_eq!(config.listen_addr, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(config.gossip_addr, "0.0.0.0:8081".parse().unwrap());
        assert!(config.seed_nodes.is_empty());
        assert_eq!(config.default_target_rate, 100.0);
        assert_eq!(config.default_cluster_target, 300.0);
        assert_eq!(config.default_kp, 0.5);
        assert_eq!(config.default_ki, 0.02);
        assert_eq!(config.default_kd, 0.08);
        assert_eq!(config.sync_interval, Duration::from_millis(500));
        assert_eq!(config.stale_timeout, Duration::from_secs(10));
    }

    #[test]
    #[serial]
    fn test_custom_gossip_timing() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_SYNC_INTERVAL_MS", "250");
        env::set_var("NENYA_STALE_TIMEOUT_MS", "5000");

        let config = Config::from_env().unwrap();
        assert_eq!(config.sync_interval, Duration::from_millis(250));
        assert_eq!(config.stale_timeout, Duration::from_millis(5000));
    }

    #[test]
    #[serial]
    fn test_stale_timeout_must_exceed_full_weight_window() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_SYNC_INTERVAL_MS", "500");
        env::set_var("NENYA_STALE_TIMEOUT_MS", "1000");

        let result = Config::from_env();
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::InvalidValue(name, _) => {
                assert_eq!(name, "NENYA_STALE_TIMEOUT_MS");
            }
            _ => panic!("Expected InvalidValue error"),
        }
    }

    #[test]
    #[serial]
    fn test_zero_sync_interval_rejected() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_SYNC_INTERVAL_MS", "0");

        let result = Config::from_env();
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::InvalidValue(name, _) => {
                assert_eq!(name, "NENYA_SYNC_INTERVAL_MS");
            }
            _ => panic!("Expected InvalidValue error"),
        }
    }

    #[test]
    #[serial]
    fn test_custom_addresses() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_LISTEN_ADDR", "0.0.0.0:9090");
        env::set_var("NENYA_GOSSIP_ADDR", "127.0.0.1:9091");

        let config = Config::from_env().unwrap();

        assert_eq!(config.listen_addr, "0.0.0.0:9090".parse().unwrap());
        assert_eq!(config.gossip_addr, "127.0.0.1:9091".parse().unwrap());
    }

    #[test]
    #[serial]
    fn test_invalid_listen_addr() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_LISTEN_ADDR", "invalid-address");

        let result = Config::from_env();
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::InvalidValue(name, _) => {
                assert_eq!(name, "NENYA_LISTEN_ADDR");
            }
            _ => panic!("Expected InvalidValue error"),
        }
    }

    #[test]
    #[serial]
    fn test_seed_nodes_parsing() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var(
            "NENYA_SEED_NODES",
            "127.0.0.1:8081,192.168.1.100:8081,10.0.0.1:9000",
        );

        let config = Config::from_env().unwrap();

        assert_eq!(config.seed_nodes.len(), 3);
        assert_eq!(config.seed_nodes[0], "127.0.0.1:8081".parse().unwrap());
        assert_eq!(config.seed_nodes[1], "192.168.1.100:8081".parse().unwrap());
        assert_eq!(config.seed_nodes[2], "10.0.0.1:9000".parse().unwrap());
    }

    #[test]
    #[serial]
    fn test_seed_nodes_with_spaces() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_SEED_NODES", " 127.0.0.1:8081 , 192.168.1.100:8081 ");

        let config = Config::from_env().unwrap();

        assert_eq!(config.seed_nodes.len(), 2);
        assert_eq!(config.seed_nodes[0], "127.0.0.1:8081".parse().unwrap());
        assert_eq!(config.seed_nodes[1], "192.168.1.100:8081".parse().unwrap());
    }

    #[test]
    #[serial]
    fn test_empty_seed_nodes() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_SEED_NODES", "");

        let config = Config::from_env().unwrap();
        assert!(config.seed_nodes.is_empty());
    }

    #[test]
    #[serial]
    fn test_invalid_seed_node() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var(
            "NENYA_SEED_NODES",
            "127.0.0.1:8081,invalid-node,10.0.0.1:9000",
        );

        let result = Config::from_env();
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::InvalidValue(name, _) => {
                assert_eq!(name, "NENYA_SEED_NODES");
            }
            _ => panic!("Expected InvalidValue error"),
        }
    }

    #[test]
    #[serial]
    fn test_custom_node_id() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_NODE_ID", "custom-node-42");

        let config = Config::from_env().unwrap();
        assert_eq!(config.node_id, "custom-node-42");
    }

    #[test]
    #[serial]
    fn test_custom_target_rates() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_DEFAULT_TARGET_RATE", "500.0");
        env::set_var("NENYA_DEFAULT_CLUSTER_TARGET", "1500.0");

        let config = Config::from_env().unwrap();
        assert_eq!(config.default_target_rate, 500.0);
        assert_eq!(config.default_cluster_target, 1500.0);
    }

    #[test]
    #[serial]
    fn test_invalid_target_rate() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_DEFAULT_TARGET_RATE", "-100.0");

        let result = Config::from_env();
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::InvalidValue(name, msg) => {
                assert_eq!(name, "NENYA_DEFAULT_TARGET_RATE");
                assert!(msg.contains("positive"));
            }
            _ => panic!("Expected InvalidValue error"),
        }
    }

    #[test]
    #[serial]
    fn test_custom_pid_parameters() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_DEFAULT_KP", "1.0");
        env::set_var("NENYA_DEFAULT_KI", "0.1");
        env::set_var("NENYA_DEFAULT_KD", "0.05");

        let config = Config::from_env().unwrap();
        assert_eq!(config.default_kp, 1.0);
        assert_eq!(config.default_ki, 0.1);
        assert_eq!(config.default_kd, 0.05);
    }

    #[test]
    #[serial]
    fn test_auto_calculated_min_max_rates() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_DEFAULT_TARGET_RATE", "200.0");

        let config = Config::from_env().unwrap();
        // Should be 0.5 * 200.0 = 100.0 and 2.0 * 200.0 = 400.0
        assert_eq!(config.default_min_rate, 100.0);
        assert_eq!(config.default_max_rate, 400.0);
    }

    #[test]
    #[serial]
    fn test_explicit_min_max_rates() {
        clean_env();
        env::set_var("NENYA_CLUSTER_SECRET", "test");
        env::set_var("NENYA_DEFAULT_TARGET_RATE", "200.0");
        env::set_var("NENYA_DEFAULT_MIN_RATE", "150.0");
        env::set_var("NENYA_DEFAULT_MAX_RATE", "500.0");

        let config = Config::from_env().unwrap();
        assert_eq!(config.default_min_rate, 150.0);
        assert_eq!(config.default_max_rate, 500.0);
    }
}
