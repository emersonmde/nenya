//! Simple, fixed-layout cluster dashboard
//!
//! Usage:
//! ```bash
//! cargo run --example cluster_visualizer -- \
//!     --nodes 127.0.0.1:8080,127.0.0.1:8090,127.0.0.1:8100 \
//!     --scope test
//! ```

use eframe::egui;
use egui::{Color32, FontId, RichText, Stroke, Vec2};
use egui_plot::{Line, Plot, PlotPoints};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const HISTORY_SIZE: usize = 120;
const POLL_INTERVAL_MS: u64 = 500;

// Fixed dimensions (like CSS)
const WINDOW_WIDTH: f32 = 1400.0;
const CARD_WIDTH: f32 = 400.0;
const CARD_HEIGHT: f32 = 220.0;
const GRAPH_HEIGHT: f32 = 300.0;

fn main() -> Result<(), eframe::Error> {
    let args: Vec<String> = std::env::args().collect();

    let nodes = if let Some(pos) = args.iter().position(|a| a == "--nodes") {
        args.get(pos + 1)
            .expect("Missing value for --nodes")
            .split(',')
            .map(|s| s.to_string())
            .collect()
    } else {
        vec![
            "127.0.0.1:8080".to_string(),
            "127.0.0.1:8090".to_string(),
            "127.0.0.1:8100".to_string(),
        ]
    };

    let scope = if let Some(pos) = args.iter().position(|a| a == "--scope") {
        args.get(pos + 1)
            .expect("Missing value for --scope")
            .to_string()
    } else {
        "test".to_string()
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([WINDOW_WIDTH, 1000.0])
            .with_title("Nenya Cluster Monitor"),
        ..Default::default()
    };

    eframe::run_native(
        "Nenya Cluster Monitor",
        options,
        Box::new(|_cc| Ok(Box::new(ClusterDashboard::new(nodes, scope)))),
    )
}

#[derive(Debug, Clone, Default)]
struct NodeMetrics {
    healthy: bool,
    peers: usize,
    accepted_rate: f64,
    refill_rate: f64,
    total_rate: f64,
    throttled_rate: f64,
    // Debug fields
    target_rate: f64,
    external_accepted_rate: f64,
}

#[derive(Debug, Clone)]
struct ClusterSnapshot {
    timestamp: f64,
    total_accepted: f64,
    total_throttled: f64,
    total_generated: f64,
}

struct ClusterDashboard {
    nodes: Vec<String>,
    scope: String,
    client: Client,
    runtime: tokio::runtime::Runtime,
    current_metrics: HashMap<usize, NodeMetrics>,
    cluster_history: VecDeque<ClusterSnapshot>,
    last_poll: Instant,
    start_time: Instant,
    target_rate: f64,
}

impl ClusterDashboard {
    fn new(nodes: Vec<String>, scope: String) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime");

        Self {
            nodes,
            scope,
            client: Client::new(),
            runtime,
            current_metrics: HashMap::new(),
            cluster_history: VecDeque::new(),
            last_poll: Instant::now(),
            start_time: Instant::now(),
            target_rate: 100.0,
        }
    }

    fn poll_all_nodes(&mut self) {
        let nodes = self.nodes.clone();
        let scope = self.scope.clone();
        let client = self.client.clone();

        let results = self.runtime.block_on(async {
            let mut handles = Vec::new();

            for (i, node_addr) in nodes.iter().enumerate() {
                let client = client.clone();
                let scope = scope.clone();
                let node_addr = node_addr.clone();

                handles.push(tokio::spawn(async move {
                    let health_result = Self::fetch_health(&client, &node_addr).await;
                    let stats_result = Self::fetch_scope_stats(&client, &node_addr, &scope).await;

                    let metrics = match (health_result, stats_result) {
                        (Ok(health), Ok(stats)) => NodeMetrics {
                            healthy: health["healthy"].as_bool().unwrap_or(false),
                            peers: health["peers"].as_u64().unwrap_or(0) as usize,
                            accepted_rate: stats["accepted_request_rate"].as_f64().unwrap_or(0.0),
                            refill_rate: stats["refill_rate"].as_f64().unwrap_or(0.0),
                            total_rate: stats["total_request_rate"].as_f64().unwrap_or(0.0),
                            throttled_rate: stats["throttled_request_rate"].as_f64().unwrap_or(0.0),
                            target_rate: stats["target_rate"].as_f64().unwrap_or(0.0),
                            external_accepted_rate: stats["external_accepted_rate"]
                                .as_f64()
                                .unwrap_or(0.0),
                        },
                        _ => NodeMetrics::default(),
                    };

                    (i, metrics)
                }));
            }

            let mut results = HashMap::new();
            for handle in handles {
                if let Ok((i, metrics)) = handle.await {
                    results.insert(i, metrics);
                }
            }
            results
        });

        self.current_metrics = results;

        let total_accepted: f64 = self.current_metrics.values().map(|m| m.accepted_rate).sum();

        let total_throttled: f64 = self
            .current_metrics
            .values()
            .map(|m| m.throttled_rate)
            .sum();

        let total_generated: f64 = self.current_metrics.values().map(|m| m.total_rate).sum();

        let elapsed = self.start_time.elapsed().as_secs_f64();

        self.cluster_history.push_back(ClusterSnapshot {
            timestamp: elapsed,
            total_accepted,
            total_throttled,
            total_generated,
        });

        while self.cluster_history.len() > HISTORY_SIZE {
            self.cluster_history.pop_front();
        }
    }

    async fn fetch_health(client: &Client, node_addr: &str) -> Result<Value, String> {
        let url = format!("http://{}/health", node_addr);
        let response = client
            .get(&url)
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

    async fn fetch_scope_stats(
        client: &Client,
        node_addr: &str,
        scope: &str,
    ) -> Result<Value, String> {
        let url = format!("http://{}/scope_stats?scope={}", node_addr, scope);
        let response = client
            .get(&url)
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

    fn health_color(&self, node_id: usize) -> Color32 {
        if let Some(metrics) = self.current_metrics.get(&node_id) {
            if !metrics.healthy {
                return Color32::from_rgb(220, 50, 50);
            }
            let expected = self.target_rate / self.nodes.len() as f64;
            let deviation = ((metrics.accepted_rate - expected) / expected).abs();

            if deviation < 0.3 {
                Color32::from_rgb(50, 220, 120)
            } else if deviation < 0.6 {
                Color32::from_rgb(255, 200, 80)
            } else {
                Color32::from_rgb(255, 120, 80)
            }
        } else {
            Color32::from_rgb(150, 150, 150)
        }
    }

    fn cluster_status(&self) -> (&str, Color32) {
        let online = self.current_metrics.len();
        let total = self.nodes.len();

        if online == 0 {
            return ("OFFLINE", Color32::from_rgb(220, 50, 50));
        }

        if online < total {
            return ("DEGRADED", Color32::from_rgb(255, 200, 80));
        }

        let total_accepted: f64 = self.current_metrics.values().map(|m| m.accepted_rate).sum();

        if total_accepted < 1.0 {
            return ("IDLE", Color32::from_rgb(150, 180, 255));
        }

        let deviation = ((total_accepted - self.target_rate) / self.target_rate).abs();

        if deviation < 0.1 {
            ("OPTIMAL", Color32::from_rgb(50, 220, 120))
        } else if deviation < 0.2 {
            ("CONVERGING", Color32::from_rgb(150, 220, 150))
        } else {
            ("ADJUSTING", Color32::from_rgb(255, 200, 80))
        }
    }
}

impl eframe::App for ClusterDashboard {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_poll.elapsed() > Duration::from_millis(POLL_INTERVAL_MS) {
            self.poll_all_nodes();
            self.last_poll = Instant::now();
        }

        let mut style = (*ctx.style()).clone();
        style.visuals.window_fill = Color32::from_rgb(12, 16, 22);
        style.visuals.panel_fill = Color32::from_rgb(12, 16, 22);
        ctx.set_style(style);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.set_width(WINDOW_WIDTH);

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(20.0);

                // Header
                self.render_header(ui);

                ui.add_space(30.0);

                // Node cards - fixed layout, 3 per row
                ui.horizontal(|ui| {
                    for i in 0..self.nodes.len() {
                        if i > 0 && i % 3 == 0 {
                            // Start new row after 3 cards
                        }
                        self.render_node_card(ui, i);
                        ui.add_space(20.0);
                    }
                });

                ui.add_space(30.0);

                // Cluster graph - fixed size
                self.render_cluster_graph(ui);

                ui.add_space(30.0);

                // Topology
                self.render_topology(ui);

                ui.add_space(20.0);
            });
        });

        ctx.request_repaint_after(Duration::from_millis(POLL_INTERVAL_MS / 2));
    }
}

impl ClusterDashboard {
    fn render_header(&self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(18, 22, 30))
            .stroke(Stroke::new(1.0, Color32::from_rgb(60, 140, 200)))
            .rounding(6.0)
            .inner_margin(20.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("NENYA CLUSTER")
                            .size(28.0)
                            .color(Color32::from_rgb(120, 200, 255))
                            .strong(),
                    );

                    ui.add_space(40.0);

                    let (status, color) = self.cluster_status();
                    ui.label(
                        RichText::new(format!("● {}", status))
                            .size(24.0)
                            .color(color)
                            .strong(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let online = self.current_metrics.len();
                        let total = self.nodes.len();
                        ui.label(
                            RichText::new(format!("{}/{} nodes", online, total))
                                .size(18.0)
                                .color(Color32::from_rgb(180, 200, 220)),
                        );
                    });
                });

                ui.add_space(12.0);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Scope:")
                            .size(14.0)
                            .color(Color32::from_rgb(140, 160, 180)),
                    );
                    ui.label(
                        RichText::new(&self.scope)
                            .size(14.0)
                            .color(Color32::from_rgb(200, 220, 255))
                            .strong(),
                    );

                    ui.add_space(30.0);

                    ui.label(
                        RichText::new("Target:")
                            .size(14.0)
                            .color(Color32::from_rgb(140, 160, 180)),
                    );
                    ui.label(
                        RichText::new(format!("{:.1} TPS", self.target_rate))
                            .size(14.0)
                            .color(Color32::from_rgb(255, 220, 120))
                            .strong(),
                    );

                    ui.add_space(30.0);

                    let total: f64 = self.current_metrics.values().map(|m| m.accepted_rate).sum();
                    ui.label(
                        RichText::new("Actual:")
                            .size(14.0)
                            .color(Color32::from_rgb(140, 160, 180)),
                    );
                    ui.label(
                        RichText::new(format!("{:.1} TPS", total))
                            .size(14.0)
                            .color(Color32::from_rgb(120, 255, 180))
                            .strong(),
                    );
                });
            });
    }

    fn render_node_card(&self, ui: &mut egui::Ui, node_id: usize) {
        let node_addr = &self.nodes[node_id];
        let metrics = self.current_metrics.get(&node_id);
        let color = self.health_color(node_id);

        egui::Frame::none()
            .fill(Color32::from_rgb(22, 28, 38))
            .stroke(Stroke::new(2.5, color))
            .rounding(8.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(CARD_WIDTH);
                ui.set_height(CARD_HEIGHT);

                ui.vertical(|ui| {
                    // Header
                    ui.horizontal(|ui| {
                        ui.heading(
                            RichText::new(format!("NODE {}", node_id))
                                .size(18.0)
                                .color(Color32::from_rgb(180, 220, 255)),
                        );
                    });

                    ui.label(
                        RichText::new(node_addr)
                            .size(10.0)
                            .color(Color32::from_rgb(120, 130, 150)),
                    );

                    ui.add_space(8.0);

                    if let Some(m) = metrics {
                        // Row 1: TOTAL, ACCEPTED, PEERS
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("TOTAL")
                                        .size(9.0)
                                        .color(Color32::from_rgb(140, 160, 180)),
                                );
                                ui.heading(
                                    RichText::new(format!("{:.1}", m.total_rate))
                                        .size(20.0)
                                        .color(Color32::from_rgb(180, 200, 255)),
                                );
                            });

                            ui.add_space(20.0);

                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("ACCEPTED")
                                        .size(9.0)
                                        .color(Color32::from_rgb(140, 160, 180)),
                                );
                                ui.heading(
                                    RichText::new(format!("{:.1}", m.accepted_rate))
                                        .size(20.0)
                                        .color(Color32::from_rgb(120, 255, 180)),
                                );
                            });

                            ui.add_space(20.0);

                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("PEERS")
                                        .size(9.0)
                                        .color(Color32::from_rgb(140, 160, 180)),
                                );
                                ui.heading(
                                    RichText::new(format!("{}", m.peers))
                                        .size(20.0)
                                        .color(Color32::from_rgb(255, 200, 120)),
                                );
                            });
                        });

                        ui.add_space(8.0);

                        // Row 2: THROTTLED and REFILL
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("THROTTLED")
                                        .size(9.0)
                                        .color(Color32::from_rgb(140, 160, 180)),
                                );
                                ui.heading(
                                    RichText::new(format!("{:.1}", m.throttled_rate))
                                        .size(20.0)
                                        .color(Color32::from_rgb(255, 180, 120)),
                                );
                            });

                            ui.add_space(20.0);

                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("REFILL")
                                        .size(9.0)
                                        .color(Color32::from_rgb(140, 160, 180)),
                                );
                                ui.heading(
                                    RichText::new(format!("{:.1}", m.refill_rate))
                                        .size(20.0)
                                        .color(Color32::from_rgb(200, 160, 255)),
                                );
                            });

                            ui.add_space(20.0);

                            // Empty space for alignment
                            ui.vertical(|ui| {
                                ui.add_space(30.0);
                            });
                        });

                        ui.add_space(8.0);

                        // Debug info
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "Target: {:.1} | External: {:.1} | Error: {:.1}",
                                    m.target_rate,
                                    m.external_accepted_rate,
                                    m.target_rate - (m.accepted_rate + m.external_accepted_rate)
                                ))
                                .size(8.0)
                                .color(Color32::from_rgb(120, 140, 160)),
                            );
                        });
                    } else {
                        ui.add_space(20.0);
                        ui.vertical_centered(|ui| {
                            ui.heading(
                                RichText::new("OFFLINE")
                                    .size(24.0)
                                    .color(Color32::from_rgb(200, 80, 80)),
                            );
                        });
                    }
                });
            });
    }

    fn render_cluster_graph(&self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(18, 22, 30))
            .stroke(Stroke::new(1.0, Color32::from_rgb(60, 100, 140)))
            .rounding(6.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.label(
                    RichText::new("CLUSTER THROUGHPUT (60s)")
                        .size(16.0)
                        .color(Color32::from_rgb(180, 220, 255))
                        .strong(),
                );

                ui.add_space(8.0);

                Plot::new("cluster_rate")
                    .height(GRAPH_HEIGHT)
                    .width(WINDOW_WIDTH - 100.0)
                    .show_axes([true, true])
                    .show_grid([true, true])
                    .legend(egui_plot::Legend::default())
                    .allow_zoom(false)
                    .allow_drag(false)
                    .allow_scroll(false)
                    .allow_boxed_zoom(false)
                    .show(ui, |plot_ui| {
                        if self.cluster_history.len() < 2 {
                            return;
                        }

                        // Generated (total) line
                        let generated: PlotPoints = self
                            .cluster_history
                            .iter()
                            .map(|s| [s.timestamp, s.total_generated])
                            .collect();

                        plot_ui.line(
                            Line::new(generated)
                                .name("Generated")
                                .color(Color32::from_rgb(180, 180, 200))
                                .width(2.0)
                                .style(egui_plot::LineStyle::dotted_loose()),
                        );

                        // Accepted line
                        let accepted: PlotPoints = self
                            .cluster_history
                            .iter()
                            .map(|s| [s.timestamp, s.total_accepted])
                            .collect();

                        plot_ui.line(
                            Line::new(accepted)
                                .name("Accepted")
                                .color(Color32::from_rgb(120, 255, 180))
                                .width(3.0),
                        );

                        // Throttled line
                        let throttled: PlotPoints = self
                            .cluster_history
                            .iter()
                            .map(|s| [s.timestamp, s.total_throttled])
                            .collect();

                        plot_ui.line(
                            Line::new(throttled)
                                .name("Throttled")
                                .color(Color32::from_rgb(255, 180, 120))
                                .width(2.5),
                        );

                        // Target line
                        if let (Some(first), Some(last)) =
                            (self.cluster_history.front(), self.cluster_history.back())
                        {
                            let target: PlotPoints = vec![
                                [first.timestamp, self.target_rate],
                                [last.timestamp, self.target_rate],
                            ]
                            .into();

                            plot_ui.line(
                                Line::new(target)
                                    .name("Target")
                                    .color(Color32::from_rgb(255, 220, 120))
                                    .width(2.0)
                                    .style(egui_plot::LineStyle::dashed_loose()),
                            );
                        }
                    });
            });
    }

    fn render_topology(&self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(18, 22, 30))
            .stroke(Stroke::new(1.0, Color32::from_rgb(60, 100, 140)))
            .rounding(6.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("NETWORK TOPOLOGY")
                            .size(16.0)
                            .color(Color32::from_rgb(180, 220, 255))
                            .strong(),
                    );
                    ui.label(
                        RichText::new("(Gossip mesh)")
                            .size(12.0)
                            .color(Color32::from_rgb(120, 140, 160)),
                    );
                });

                ui.add_space(8.0);

                let (response, painter) = ui
                    .allocate_painter(Vec2::new(WINDOW_WIDTH - 100.0, 250.0), egui::Sense::hover());

                let rect = response.rect;
                let center = rect.center();
                let radius = 80.0;
                let node_count = self.nodes.len();

                // Draw connections
                for i in 0..node_count {
                    for j in (i + 1)..node_count {
                        let angle_i = (i as f32 / node_count as f32) * 2.0 * std::f32::consts::PI
                            - std::f32::consts::FRAC_PI_2;
                        let angle_j = (j as f32 / node_count as f32) * 2.0 * std::f32::consts::PI
                            - std::f32::consts::FRAC_PI_2;

                        let pos_i =
                            center + Vec2::new(angle_i.cos() * radius, angle_i.sin() * radius);
                        let pos_j =
                            center + Vec2::new(angle_j.cos() * radius, angle_j.sin() * radius);

                        painter.line_segment(
                            [pos_i, pos_j],
                            Stroke::new(1.5, Color32::from_rgb(50, 120, 180)),
                        );
                    }
                }

                // Draw nodes
                for i in 0..node_count {
                    let angle = (i as f32 / node_count as f32) * 2.0 * std::f32::consts::PI
                        - std::f32::consts::FRAC_PI_2;
                    let pos = center + Vec2::new(angle.cos() * radius, angle.sin() * radius);

                    let color = self.health_color(i);

                    painter.circle_filled(pos, 14.0, color);
                    painter.circle_stroke(
                        pos,
                        14.0,
                        Stroke::new(2.0, Color32::from_rgb(255, 255, 255)),
                    );

                    painter.text(
                        pos + Vec2::new(0.0, -32.0),
                        egui::Align2::CENTER_CENTER,
                        format!("Node {}", i),
                        FontId::proportional(13.0),
                        Color32::from_rgb(220, 220, 220),
                    );

                    if let Some(metrics) = self.current_metrics.get(&i) {
                        painter.text(
                            pos + Vec2::new(0.0, 32.0),
                            egui::Align2::CENTER_CENTER,
                            format!("{:.1} TPS", metrics.accepted_rate),
                            FontId::proportional(11.0),
                            Color32::from_rgb(180, 220, 255),
                        );
                    }
                }
            });
    }
}
