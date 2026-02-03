//! Cluster load generator for testing distributed rate limiting
//!
//! Generates various load patterns across multiple nodes:
//! - Constant: Even distribution
//! - Uneven: Different TPS per node
//! - Ramp: Gradual increase/decrease
//! - Burst: Periodic spikes
//!
//! Usage:
//! ```bash
//! # Constant load: 150 TPS total across 3 nodes
//! cargo run --example cluster_load_generator -- \
//!     --nodes 127.0.0.1:8080,127.0.0.1:8090,127.0.0.1:8100 \
//!     --pattern constant \
//!     --total-tps 150 \
//!     --duration 60
//!
//! # Uneven load: 80 + 40 + 30 = 150 TPS total
//! cargo run --example cluster_load_generator -- \
//!     --nodes 127.0.0.1:8080,127.0.0.1:8090,127.0.0.1:8100 \
//!     --pattern uneven \
//!     --tps-per-node 80,40,30 \
//!     --duration 60
//! ```

use clap::{Parser, ValueEnum};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::time::sleep;

#[derive(Parser, Debug)]
#[command(name = "cluster-load-generator")]
#[command(about = "Generate load patterns for Nenya cluster testing")]
struct Args {
    /// Node addresses (comma-separated)
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "127.0.0.1:8080,127.0.0.1:8090,127.0.0.1:8100"
    )]
    nodes: Vec<String>,

    /// Load pattern to generate
    #[arg(long, default_value = "constant")]
    pattern: LoadPattern,

    /// Total TPS to generate (for constant/ramp patterns)
    #[arg(long, default_value_t = 150.0)]
    total_tps: f64,

    /// TPS per node (comma-separated, for uneven pattern)
    #[arg(long, value_delimiter = ',')]
    tps_per_node: Vec<f64>,

    /// Duration in seconds
    #[arg(long, default_value_t = 60)]
    duration: u64,

    /// Scope to test
    #[arg(long, default_value = "test")]
    scope: String,

    /// Show detailed per-node stats
    #[arg(long)]
    verbose: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LoadPattern {
    /// Even distribution across all nodes
    Constant,
    /// Different TPS per node
    Uneven,
    /// Gradual increase
    Ramp,
    /// Periodic spikes
    Burst,
    /// Sine wave oscillation (tests PID response to smooth changes)
    Sine,
    /// Step changes (tests PID response to sudden changes)
    Step,
    /// Sawtooth wave (ramp up then sudden drop)
    Sawtooth,
}

#[derive(Debug, Clone, Default)]
struct NodeStats {
    sent: u64,
    accepted: u64,
    throttled: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    println!("Cluster Load Generator");
    println!("─────────────────────────────────────────");
    println!("Pattern:     {:?}", args.pattern);
    println!("Nodes:       {}", args.nodes.len());
    println!("Duration:    {}s", args.duration);
    println!("Scope:       {}", args.scope);
    println!();

    let generator = LoadGenerator::new(args.nodes, args.scope);

    match args.pattern {
        LoadPattern::Constant => {
            generator
                .generate_constant(args.total_tps, args.duration, args.verbose)
                .await?;
        }
        LoadPattern::Uneven => {
            if args.tps_per_node.is_empty() {
                eprintln!("Error: --tps-per-node required for uneven pattern");
                std::process::exit(1);
            }
            generator
                .generate_uneven(&args.tps_per_node, args.duration, args.verbose)
                .await?;
        }
        LoadPattern::Ramp => {
            generator
                .generate_ramp(args.total_tps, args.duration, args.verbose)
                .await?;
        }
        LoadPattern::Burst => {
            generator
                .generate_burst(args.total_tps, args.duration, args.verbose)
                .await?;
        }
        LoadPattern::Sine => {
            generator
                .generate_sine(args.total_tps, args.duration, args.verbose)
                .await?;
        }
        LoadPattern::Step => {
            generator
                .generate_step(args.total_tps, args.duration, args.verbose)
                .await?;
        }
        LoadPattern::Sawtooth => {
            generator
                .generate_sawtooth(args.total_tps, args.duration, args.verbose)
                .await?;
        }
    }

    Ok(())
}

struct LoadGenerator {
    nodes: Vec<String>,
    scope: String,
    client: Client,
}

impl LoadGenerator {
    fn new(nodes: Vec<String>, scope: String) -> Self {
        Self {
            nodes,
            scope,
            client: Client::new(),
        }
    }

    async fn generate_constant(
        &self,
        total_tps: f64,
        duration_secs: u64,
        verbose: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let tps_per_node = total_tps / self.nodes.len() as f64;

        println!(
            "Total TPS:   {:.1} ({:.1} per node)",
            total_tps, tps_per_node
        );
        println!();

        let mut node_stats = vec![NodeStats::default(); self.nodes.len()];
        let start = Instant::now();
        let duration = Duration::from_secs(duration_secs);

        while start.elapsed() < duration {
            let progress = start.elapsed().as_secs_f64() / duration.as_secs_f64();
            self.print_progress(progress, &node_stats);

            // Send requests to all nodes
            for (i, node) in self.nodes.iter().enumerate() {
                let response = self.send_request(node).await;
                self.update_stats(&mut node_stats[i], response);
            }

            // Sleep to maintain TPS
            let delay = Duration::from_secs_f64(1.0 / tps_per_node);
            sleep(delay).await;
        }

        self.print_final_stats(&node_stats, verbose);

        Ok(())
    }

    async fn generate_uneven(
        &self,
        tps_per_node: &[f64],
        duration_secs: u64,
        verbose: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if tps_per_node.len() != self.nodes.len() {
            return Err(format!(
                "TPS per node count ({}) must match node count ({})",
                tps_per_node.len(),
                self.nodes.len()
            )
            .into());
        }

        let total_tps: f64 = tps_per_node.iter().sum();
        println!("Total TPS:   {:.1}", total_tps);
        for (i, tps) in tps_per_node.iter().enumerate() {
            println!("  Node {}: {:.1} TPS", i, tps);
        }
        println!();

        let mut node_stats = vec![NodeStats::default(); self.nodes.len()];
        let start = Instant::now();
        let duration = Duration::from_secs(duration_secs);

        while start.elapsed() < duration {
            let progress = start.elapsed().as_secs_f64() / duration.as_secs_f64();
            self.print_progress(progress, &node_stats);

            // Send requests based on per-node TPS
            for (i, node) in self.nodes.iter().enumerate() {
                let node_tps = tps_per_node[i];
                let delay = Duration::from_secs_f64(1.0 / node_tps);

                let response = self.send_request(node).await;
                self.update_stats(&mut node_stats[i], response);

                sleep(delay).await;
            }
        }

        self.print_final_stats(&node_stats, verbose);

        Ok(())
    }

    async fn generate_ramp(
        &self,
        max_tps: f64,
        duration_secs: u64,
        verbose: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("Max TPS:     {:.1}", max_tps);
        println!("Ramp:        0 → {:.1} TPS", max_tps);
        println!();

        let mut node_stats = vec![NodeStats::default(); self.nodes.len()];
        let start = Instant::now();
        let duration = Duration::from_secs(duration_secs);

        while start.elapsed() < duration {
            let progress = start.elapsed().as_secs_f64() / duration.as_secs_f64();
            self.print_progress(progress, &node_stats);

            // Ramp up TPS over time
            let current_tps = max_tps * progress;
            let tps_per_node = current_tps / self.nodes.len() as f64;

            if tps_per_node > 0.1 {
                for (i, node) in self.nodes.iter().enumerate() {
                    let response = self.send_request(node).await;
                    self.update_stats(&mut node_stats[i], response);
                }

                let delay = Duration::from_secs_f64(1.0 / tps_per_node.max(0.1));
                sleep(delay).await;
            } else {
                sleep(Duration::from_millis(100)).await;
            }
        }

        self.print_final_stats(&node_stats, verbose);

        Ok(())
    }

    async fn generate_burst(
        &self,
        base_tps: f64,
        duration_secs: u64,
        verbose: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let burst_tps = base_tps * 5.0;

        println!("Base TPS:    {:.1}", base_tps);
        println!("Burst TPS:   {:.1}", burst_tps);
        println!("Burst every: 10s");
        println!();

        let mut node_stats = vec![NodeStats::default(); self.nodes.len()];
        let start = Instant::now();
        let duration = Duration::from_secs(duration_secs);

        while start.elapsed() < duration {
            let progress = start.elapsed().as_secs_f64() / duration.as_secs_f64();
            self.print_progress(progress, &node_stats);

            // Burst every 10 seconds
            let in_burst = (start.elapsed().as_secs() % 10) < 2;
            let current_tps = if in_burst { burst_tps } else { base_tps };
            let tps_per_node = current_tps / self.nodes.len() as f64;

            for (i, node) in self.nodes.iter().enumerate() {
                let response = self.send_request(node).await;
                self.update_stats(&mut node_stats[i], response);
            }

            let delay = Duration::from_secs_f64(1.0 / tps_per_node);
            sleep(delay).await;
        }

        self.print_final_stats(&node_stats, verbose);

        Ok(())
    }

    async fn send_request(&self, node: &str) -> Result<Value, String> {
        let url = format!("http://{}/should_throttle", node);

        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({ "scope": self.scope }))
            .timeout(Duration::from_millis(500))
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if response.status() != StatusCode::OK {
            return Err(format!("Status: {}", response.status()));
        }

        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {}", e))
    }

    fn update_stats(&self, stats: &mut NodeStats, response: Result<Value, String>) {
        stats.sent += 1;

        if let Ok(resp) = response {
            if let Some(should_throttle) = resp["should_throttle"].as_bool() {
                if should_throttle {
                    stats.throttled += 1;
                } else {
                    stats.accepted += 1;
                }
            }
        }
    }

    fn print_progress(&self, progress: f64, stats: &[NodeStats]) {
        let bar_width = 40;
        let filled = (progress * bar_width as f64) as usize;
        let empty = bar_width - filled;

        let bar = format!(
            "[{}{}] {:.0}%",
            "█".repeat(filled),
            "░".repeat(empty),
            progress * 100.0
        );

        let total_sent: u64 = stats.iter().map(|s| s.sent).sum();
        let total_accepted: u64 = stats.iter().map(|s| s.accepted).sum();

        print!(
            "\rProgress: {}  Sent: {}  Accepted: {}  ",
            bar, total_sent, total_accepted
        );
        use std::io::Write;
        std::io::stdout().flush().unwrap();
    }

    fn print_final_stats(&self, stats: &[NodeStats], verbose: bool) {
        println!("\n");
        println!("Final Statistics");
        println!("─────────────────────────────────────────");

        if verbose {
            for (i, stat) in stats.iter().enumerate() {
                let accept_rate = if stat.sent > 0 {
                    stat.accepted as f64 / stat.sent as f64 * 100.0
                } else {
                    0.0
                };

                println!(
                    "Node {}:  Sent: {:5}  Accepted: {:5} ({:.1}%)  Throttled: {:5}",
                    i, stat.sent, stat.accepted, accept_rate, stat.throttled
                );
            }
            println!();
        }

        let total_sent: u64 = stats.iter().map(|s| s.sent).sum();
        let total_accepted: u64 = stats.iter().map(|s| s.accepted).sum();
        let total_throttled: u64 = stats.iter().map(|s| s.throttled).sum();

        let accept_rate = if total_sent > 0 {
            total_accepted as f64 / total_sent as f64 * 100.0
        } else {
            0.0
        };

        println!(
            "Cluster: Sent: {}  Accepted: {} ({:.1}%)  Throttled: {}",
            total_sent, total_accepted, accept_rate, total_throttled
        );
    }

    /// Generate sine wave pattern - oscillates between 50% and 150% of target
    /// Tests PID response to smooth, predictable changes
    async fn generate_sine(
        &self,
        base_tps: f64,
        duration_secs: u64,
        verbose: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("Base TPS:    {:.1} (oscillates ±50%)", base_tps);
        println!("Period:      20 seconds");
        println!();

        let mut node_stats = vec![NodeStats::default(); self.nodes.len()];
        let start = Instant::now();
        let duration = Duration::from_secs(duration_secs);

        while start.elapsed() < duration {
            let progress = start.elapsed().as_secs_f64() / duration.as_secs_f64();
            self.print_progress(progress, &node_stats);

            let elapsed_secs = start.elapsed().as_secs_f64();

            // Sine wave: oscillates between 0.5x and 1.5x base TPS with 20s period
            let phase = (elapsed_secs * 2.0 * std::f64::consts::PI / 20.0).sin();
            let current_tps = base_tps * (1.0 + 0.5 * phase);
            let tps_per_node = current_tps / self.nodes.len() as f64;

            if tps_per_node > 0.1 {
                for (i, node) in self.nodes.iter().enumerate() {
                    let response = self.send_request(node).await;
                    self.update_stats(&mut node_stats[i], response);
                }

                let delay = Duration::from_secs_f64(1.0 / tps_per_node);
                sleep(delay).await;
            } else {
                sleep(Duration::from_millis(100)).await;
            }
        }

        self.print_final_stats(&node_stats, verbose);
        Ok(())
    }

    /// Generate step pattern - sudden changes every 15 seconds
    /// Tests PID response to discontinuous changes
    async fn generate_step(
        &self,
        base_tps: f64,
        duration_secs: u64,
        verbose: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("Base TPS:    {:.1}", base_tps);
        println!("Pattern:     Steps every 15s: 0.5x → 1.0x → 1.5x → 1.0x → ...");
        println!();

        let mut node_stats = vec![NodeStats::default(); self.nodes.len()];
        let start = Instant::now();
        let duration = Duration::from_secs(duration_secs);

        let steps = [0.5, 1.0, 1.5, 1.0]; // Multipliers
        let step_duration = 15; // seconds per step

        while start.elapsed() < duration {
            let progress = start.elapsed().as_secs_f64() / duration.as_secs_f64();
            self.print_progress(progress, &node_stats);

            let elapsed_secs = start.elapsed().as_secs();
            let step_index = (elapsed_secs / step_duration) % steps.len() as u64;
            let multiplier = steps[step_index as usize];
            let current_tps = base_tps * multiplier;
            let tps_per_node = current_tps / self.nodes.len() as f64;

            if tps_per_node > 0.1 {
                for (i, node) in self.nodes.iter().enumerate() {
                    let response = self.send_request(node).await;
                    self.update_stats(&mut node_stats[i], response);
                }

                let delay = Duration::from_secs_f64(1.0 / tps_per_node);
                sleep(delay).await;
            } else {
                sleep(Duration::from_millis(100)).await;
            }
        }

        self.print_final_stats(&node_stats, verbose);
        Ok(())
    }

    /// Generate sawtooth pattern - linear ramp up then sudden drop
    /// Tests PID response to gradual increase followed by shock
    async fn generate_sawtooth(
        &self,
        max_tps: f64,
        duration_secs: u64,
        verbose: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("Max TPS:     {:.1}", max_tps);
        println!("Pattern:     Ramps from 50% to 150% over 30s, then drops instantly");
        println!();

        let mut node_stats = vec![NodeStats::default(); self.nodes.len()];
        let start = Instant::now();
        let duration = Duration::from_secs(duration_secs);

        let cycle_duration = 30.0; // seconds

        while start.elapsed() < duration {
            let progress = start.elapsed().as_secs_f64() / duration.as_secs_f64();
            self.print_progress(progress, &node_stats);

            let elapsed_secs = start.elapsed().as_secs_f64();
            let cycle_position = (elapsed_secs % cycle_duration) / cycle_duration;

            // Ramp from 0.5x to 1.5x linearly
            let current_tps = max_tps * (0.5 + cycle_position);
            let tps_per_node = current_tps / self.nodes.len() as f64;

            if tps_per_node > 0.1 {
                for (i, node) in self.nodes.iter().enumerate() {
                    let response = self.send_request(node).await;
                    self.update_stats(&mut node_stats[i], response);
                }

                let delay = Duration::from_secs_f64(1.0 / tps_per_node);
                sleep(delay).await;
            } else {
                sleep(Duration::from_millis(100)).await;
            }
        }

        self.print_final_stats(&node_stats, verbose);
        Ok(())
    }
}
