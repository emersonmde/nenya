//! Cluster test harness for multi-node integration tests

use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use tokio::time::sleep;

/// Port allocator for test nodes
static NEXT_PORT_BASE: AtomicU16 = AtomicU16::new(12000);

/// Allocate a unique port pair (HTTP + gossip) for a test node
fn allocate_port_pair() -> (u16, u16) {
    let base = NEXT_PORT_BASE.fetch_add(10, Ordering::SeqCst);
    (base, base + 1)
}

/// Handle for a single cluster node
#[allow(dead_code)]
pub struct NodeHandle {
    pub node_id: String,
    pub http_port: u16,
    pub gossip_port: u16,
    process: Option<Child>,
}

impl NodeHandle {
    /// Get the HTTP base URL for this node
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.http_port, path)
    }
}

impl Drop for NodeHandle {
    fn drop(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

/// Cluster test harness for spawning and managing multiple nodes
pub struct ClusterTestHarness {
    pub nodes: Vec<NodeHandle>,
    client: Client,
}

impl ClusterTestHarness {
    /// Spawn a cluster with the specified number of nodes
    pub async fn spawn_cluster(count: usize) -> Result<Self, String> {
        if count == 0 {
            return Err("Cluster must have at least 1 node".to_string());
        }

        let mut nodes = Vec::new();
        let mut seed_nodes = Vec::new();

        // First node is the seed
        let (http_port, gossip_port) = allocate_port_pair();
        let node_id = "node-0".to_string();

        let process = spawn_node(&node_id, http_port, gossip_port, &[])
            .map_err(|e| format!("Failed to spawn node 0: {}", e))?;

        seed_nodes.push(format!("127.0.0.1:{}", gossip_port));

        nodes.push(NodeHandle {
            node_id: node_id.clone(),
            http_port,
            gossip_port,
            process: Some(process),
        });

        // Wait for first node to be ready
        wait_for_node_ready(http_port, Duration::from_secs(10))
            .await
            .map_err(|e| format!("Node 0 failed to start: {}", e))?;

        // Spawn remaining nodes
        for i in 1..count {
            let (http_port, gossip_port) = allocate_port_pair();
            let node_id = format!("node-{}", i);

            let process = spawn_node(&node_id, http_port, gossip_port, &seed_nodes)
                .map_err(|e| format!("Failed to spawn node {}: {}", i, e))?;

            nodes.push(NodeHandle {
                node_id: node_id.clone(),
                http_port,
                gossip_port,
                process: Some(process),
            });

            // Wait for node to be ready
            wait_for_node_ready(http_port, Duration::from_secs(10))
                .await
                .map_err(|e| format!("Node {} failed to start: {}", i, e))?;
        }

        Ok(ClusterTestHarness {
            nodes,
            client: Client::new(),
        })
    }

    /// Wait for cluster convergence (all nodes see expected peer count)
    pub async fn wait_for_convergence(&self, timeout: Duration) -> Result<(), String> {
        let expected_peers = self.nodes.len() - 1;
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                return Err(format!("Cluster failed to converge within {:?}", timeout));
            }

            let mut all_converged = true;

            for node in &self.nodes {
                match self.get_health(node.http_port).await {
                    Ok(health) => {
                        if health["peers"].as_u64().unwrap_or(0) as usize != expected_peers {
                            all_converged = false;
                            break;
                        }
                    }
                    Err(_) => {
                        all_converged = false;
                        break;
                    }
                }
            }

            if all_converged {
                return Ok(());
            }

            sleep(Duration::from_millis(200)).await;
        }
    }

    /// Make a throttle request to a specific node
    pub async fn should_throttle(&self, node_idx: usize, scope: &str) -> Result<Value, String> {
        if node_idx >= self.nodes.len() {
            return Err(format!("Node index {} out of range", node_idx));
        }

        let url = self.nodes[node_idx].url("/should_throttle");

        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({ "scope": scope }))
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if response.status() != StatusCode::OK {
            return Err(format!("Request returned status: {}", response.status()));
        }

        response
            .json()
            .await
            .map_err(|e| format!("Failed to parse JSON: {}", e))
    }

    /// Get health status from a node
    pub async fn get_health(&self, port: u16) -> Result<Value, String> {
        let url = format!("http://127.0.0.1:{}/health", port);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        response
            .json()
            .await
            .map_err(|e| format!("Failed to parse JSON: {}", e))
    }

    /// Get cluster-wide total accepted rate for a scope
    #[allow(dead_code)]
    pub async fn get_cluster_total_accepted_rate(&self, scope: &str) -> Result<f64, String> {
        let mut total = 0.0;

        for node in &self.nodes {
            let response = self
                .should_throttle(
                    self.nodes
                        .iter()
                        .position(|n| n.http_port == node.http_port)
                        .unwrap(),
                    scope,
                )
                .await?;

            if let Some(rate) = response["local_accepted_rate"].as_f64() {
                total += rate;
            }
        }

        Ok(total)
    }

    /// Generate load across all nodes
    pub async fn generate_load(
        &self,
        scope: &str,
        requests_per_node: usize,
    ) -> Result<LoadStats, String> {
        let mut stats = LoadStats {
            total_requests: 0,
            accepted: 0,
            throttled: 0,
        };

        for node_idx in 0..self.nodes.len() {
            for _ in 0..requests_per_node {
                let response = self.should_throttle(node_idx, scope).await?;

                stats.total_requests += 1;

                if let Some(should_throttle) = response["should_throttle"].as_bool() {
                    if should_throttle {
                        stats.throttled += 1;
                    } else {
                        stats.accepted += 1;
                    }
                }
            }
        }

        Ok(stats)
    }
}

/// Load generation statistics
#[derive(Debug, Clone)]
pub struct LoadStats {
    pub total_requests: usize,
    pub accepted: usize,
    pub throttled: usize,
}

/// Spawn a single node process
fn spawn_node(
    node_id: &str,
    http_port: u16,
    gossip_port: u16,
    seed_nodes: &[String],
) -> Result<Child, std::io::Error> {
    let mut cmd = Command::new("cargo");
    cmd.args(["run", "--features", "server", "--quiet", "--"])
        .env("NENYA_CLUSTER_SECRET", "test-cluster-secret")
        .env("NENYA_LISTEN_ADDR", format!("127.0.0.1:{}", http_port))
        .env("NENYA_GOSSIP_ADDR", format!("127.0.0.1:{}", gossip_port))
        .env("NENYA_NODE_ID", node_id)
        .env("NENYA_DEFAULT_TARGET_RATE", "100.0")
        .env("NENYA_DEFAULT_CLUSTER_TARGET", "300.0");

    if !seed_nodes.is_empty() {
        cmd.env("NENYA_SEED_NODES", seed_nodes.join(","));
    }

    cmd.spawn()
}

/// Wait for a node to be ready
async fn wait_for_node_ready(port: u16, timeout: Duration) -> Result<(), String> {
    let client = Client::new();
    let url = format!("http://127.0.0.1:{}/health", port);
    let start = std::time::Instant::now();

    loop {
        if start.elapsed() > timeout {
            return Err("Node failed to become ready within timeout".to_string());
        }

        match client.get(&url).send().await {
            Ok(response) if response.status().is_success() => return Ok(()),
            _ => sleep(Duration::from_millis(100)).await,
        }
    }
}
