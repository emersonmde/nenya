//! Deterministic multi-node cluster simulator CLI (Milestone 4).
//!
//! Runs library scenarios, writes CSV/JSON time series and optional SVG
//! charts, and emits a scenario-matrix comparison table — the benchmark
//! harness Milestone 5 uses to judge control engines.
//!
//! ```bash
//! cargo run --features sim --example cluster_sim -- --list
//! cargo run --features sim --example cluster_sim -- --scenario partition --seed 42 --plot
//! cargo run --features sim --example cluster_sim -- --matrix --seed 42
//! # Re-run with the same seed: byte-identical artifacts
//! ```

use std::fs;
use std::path::PathBuf;

use clap::Parser;
use plotters::prelude::*;

use nenya::sim::{scenario, BayesianParams, EngineKind, RunResult, Scenario};

#[derive(Parser)]
#[command(about = "Deterministic multi-node rate limiter simulator")]
struct Args {
    /// Scenario name (see --list)
    #[arg(long)]
    scenario: Option<String>,

    /// RNG seed; same seed + same scenario = identical output
    #[arg(long, default_value_t = 42)]
    seed: u64,

    /// Output directory for CSV/JSON/SVG artifacts
    #[arg(long, default_value = "target/sim")]
    out: PathBuf,

    /// Also render an SVG chart of the run
    #[arg(long)]
    plot: bool,

    /// List available scenarios
    #[arg(long)]
    list: bool,

    /// Run every library scenario and print a comparison table (markdown)
    #[arg(long)]
    matrix: bool,

    /// Override PID proportional gain for all runs (A/B comparisons)
    #[arg(long)]
    kp: Option<f64>,

    /// Override PID integral gain
    #[arg(long)]
    ki: Option<f64>,

    /// Override PID derivative gain
    #[arg(long)]
    kd: Option<f64>,

    /// Override the integral anti-windup clamp (fraction of cluster target);
    /// negative disables the clamp
    #[arg(long)]
    error_limit_frac: Option<f64>,

    /// Control engine: pid | bayesian | hybrid. For --matrix, omit to run
    /// all three (one table per engine).
    #[arg(long)]
    engine: Option<String>,

    /// Override estimator process noise q (bayesian/hybrid)
    #[arg(long)]
    process_noise: Option<f64>,

    /// Override estimator measurement noise r (bayesian/hybrid)
    #[arg(long)]
    measurement_noise: Option<f64>,

    /// Override admission confidence z (bayesian)
    #[arg(long)]
    confidence_z: Option<f64>,
}

fn apply_overrides(mut s: Scenario, args: &Args, engine: EngineKind) -> Scenario {
    s.cfg.engine = engine;
    if let Some(kp) = args.kp {
        s.cfg.kp = kp;
    }
    if let Some(ki) = args.ki {
        s.cfg.ki = ki;
    }
    if let Some(kd) = args.kd {
        s.cfg.kd = kd;
    }
    if let Some(frac) = args.error_limit_frac {
        s.cfg.error_limit_frac = if frac < 0.0 { None } else { Some(frac) };
    }
    if args.process_noise.is_some()
        || args.measurement_noise.is_some()
        || args.confidence_z.is_some()
    {
        let mut p = match engine {
            EngineKind::Hybrid => BayesianParams::hybrid_default(),
            _ => BayesianParams::default(),
        };
        if let Some(q) = args.process_noise {
            p.process_noise = q;
        }
        if let Some(r) = args.measurement_noise {
            p.measurement_noise = r;
        }
        if let Some(z) = args.confidence_z {
            p.confidence_z = z;
        }
        s.cfg.estimator = Some(p);
    }
    s
}

/// Engines this invocation runs: the explicit one, or (matrix mode) all.
fn selected_engines(args: &Args) -> Vec<EngineKind> {
    match args.engine.as_deref() {
        Some(s) => match s.parse() {
            Ok(kind) => vec![kind],
            Err(e) => {
                eprintln!("{}", e);
                std::process::exit(2);
            }
        },
        None if args.matrix => vec![EngineKind::Pid, EngineKind::Bayesian, EngineKind::Hybrid],
        None => vec![EngineKind::Pid],
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    if args.list {
        println!("Available scenarios:");
        for s in scenario::library() {
            println!(
                "  {:<14} {} nodes, target {:.0} rps, {}s",
                s.name,
                s.cfg.num_nodes,
                s.cfg.cluster_target,
                s.duration.as_secs()
            );
        }
        return Ok(());
    }

    if args.matrix {
        run_matrix(&args);
        return Ok(());
    }

    let Some(name) = args.scenario.as_deref() else {
        eprintln!("--scenario is required (or use --list / --matrix)");
        std::process::exit(2);
    };
    let Some(s) = scenario::by_name(name) else {
        eprintln!("unknown scenario '{}'; use --list", name);
        std::process::exit(2);
    };
    let engine = selected_engines(&args)[0];
    let s = apply_overrides(s, &args, engine);

    let result = s.run(args.seed);
    fs::create_dir_all(&args.out)?;

    let base = args.out.join(format!(
        "{}_{}_seed{}",
        result.scenario, engine, result.seed
    ));
    let csv_path = base.with_extension("csv");
    let json_path = base.with_extension("json");
    fs::write(&csv_path, result.to_csv())?;
    fs::write(&json_path, result.to_json())?;
    println!("wrote {}", csv_path.display());
    println!("wrote {}", json_path.display());

    if args.plot {
        let svg_path = base.with_extension("svg");
        render_chart(&result, &svg_path)?;
        println!("wrote {}", svg_path.display());
    }

    print_summary(&result);
    Ok(())
}

fn print_summary(r: &RunResult) {
    let s = &r.summary;
    println!(
        "\n{} (seed {}, target {:.0} rps, {} nodes)",
        r.scenario, r.seed, r.target, r.num_nodes
    );
    println!("  max overshoot:         {:>10.1} rps", s.max_overshoot);
    println!(
        "  integrated overshoot:  {:>10.1} requests",
        s.integrated_overshoot
    );
    println!(
        "  integrated undershoot: {:>10.1} requests",
        s.integrated_undershoot
    );
    println!("  steady-state stddev:   {:>10.1} rps", s.steady_stddev);
    println!(
        "  steady peak-to-peak:   {:>10.1} rps",
        s.steady_peak_to_peak
    );
    match s.fairness_cv {
        Some(cv) => println!("  fairness (CV):         {:>10.3}", cv),
        None => println!("  fairness (CV):                n/a"),
    }
    for c in &s.convergence {
        let t = c
            .time_to_converge
            .map(|t| format!("{:.1}s", t))
            .unwrap_or_else(|| "never".to_string());
        println!("  convergence after {:<20} {}", format!("{}:", c.label), t);
    }
}

fn run_matrix(args: &Args) {
    for (i, engine) in selected_engines(args).into_iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("### engine: {}\n", engine);
        println!(
            "| scenario | max overshoot (rps) | overshoot (req) | undershoot (req) | converge (s) | steady stddev (rps) | fairness CV |"
        );
        println!("|---|---|---|---|---|---|---|");
        for s in scenario::library() {
            let s = apply_overrides(s, args, engine);
            let r = s.run(args.seed);
            let sm = &r.summary;
            let converge = sm
                .convergence
                .first()
                .and_then(|c| c.time_to_converge)
                .map(|t| format!("{:.1}", t))
                .unwrap_or_else(|| "never".to_string());
            let fairness = sm
                .fairness_cv
                .map(|cv| format!("{:.3}", cv))
                .unwrap_or_else(|| "n/a".to_string());
            println!(
                "| {} | {:.1} | {:.1} | {:.1} | {} | {:.1} | {} |",
                r.scenario,
                sm.max_overshoot,
                sm.integrated_overshoot,
                sm.integrated_undershoot,
                converge,
                sm.steady_stddev,
                fairness
            );
        }
    }
}

// ===== Chart rendering =====
//
// Static light-mode SVG artifact for PRs/docs. Colors are a validated
// palette (categorical slots pass CVD-separation and contrast checks on
// this surface): blue = accepted rate, orange = offered load; the target is
// a dashed neutral reference line (chrome, not a series) and per-node rates
// are recessive context lines in a pale blue — node identity is
// deliberately not encoded, since a scale-sweep run has up to 50 nodes and
// categorical hues are never cycled.

const SURFACE: RGBColor = RGBColor(0xfc, 0xfc, 0xfb);
const GRID: RGBColor = RGBColor(0xe1, 0xe0, 0xd9);
const AXIS: RGBColor = RGBColor(0xc3, 0xc2, 0xb7);
const MUTED: RGBColor = RGBColor(0x89, 0x87, 0x81);
const INK: RGBColor = RGBColor(0x0b, 0x0b, 0x0b);
const SECONDARY_INK: RGBColor = RGBColor(0x52, 0x51, 0x4e);
const ACCEPTED_BLUE: RGBColor = RGBColor(0x2a, 0x78, 0xd6);
const OFFERED_ORANGE: RGBColor = RGBColor(0xeb, 0x68, 0x34);
const NODE_PALE_BLUE: RGBColor = RGBColor(0x9e, 0xc5, 0xf4);

fn render_chart(r: &RunResult, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let t_max = r.samples.last().map(|s| s.t).unwrap_or(1.0);
    let y_max = r
        .samples
        .iter()
        .map(|s| s.offered_rate.max(s.accepted_rate))
        .fold(r.target, f64::max)
        * 1.1;

    let root = SVGBackend::new(path, (1000, 520)).into_drawing_area();
    root.fill(&SURFACE)?;

    let caption = format!("{} — seed {}", r.scenario, r.seed);
    let mut chart = ChartBuilder::on(&root)
        .margin(16)
        .caption(caption, ("sans-serif", 20).into_font().color(&INK))
        .x_label_area_size(44)
        .y_label_area_size(56)
        .build_cartesian_2d(0.0..t_max, 0.0..y_max)?;

    chart
        .configure_mesh()
        .light_line_style(GRID.mix(0.5))
        .bold_line_style(GRID)
        .axis_style(AXIS)
        .label_style(("sans-serif", 13).into_font().color(&MUTED))
        .x_desc("simulated time (s)")
        .y_desc("requests / second")
        .draw()?;

    // Per-node accepted rates: recessive context behind the cluster series
    for node in 0..r.num_nodes {
        chart.draw_series(LineSeries::new(
            r.samples.iter().map(|s| (s.t, s.per_node_rate[node])),
            NODE_PALE_BLUE.stroke_width(1),
        ))?;
    }
    // One legend entry stands in for all per-node lines
    chart
        .draw_series(std::iter::empty::<PathElement<(f64, f64)>>())?
        .label("per-node accepted")
        .legend(|(x, y)| {
            PathElement::new(vec![(x, y), (x + 16, y)], NODE_PALE_BLUE.stroke_width(1))
        });

    // Offered load
    chart
        .draw_series(LineSeries::new(
            r.samples.iter().map(|s| (s.t, s.offered_rate)),
            OFFERED_ORANGE.stroke_width(2),
        ))?
        .label("offered load")
        .legend(|(x, y)| {
            PathElement::new(vec![(x, y), (x + 16, y)], OFFERED_ORANGE.stroke_width(2))
        });

    // Cluster accepted rate
    chart
        .draw_series(LineSeries::new(
            r.samples.iter().map(|s| (s.t, s.accepted_rate)),
            ACCEPTED_BLUE.stroke_width(2),
        ))?
        .label("accepted (cluster)")
        .legend(|(x, y)| {
            PathElement::new(vec![(x, y), (x + 16, y)], ACCEPTED_BLUE.stroke_width(2))
        });

    // Target reference line (dashed, neutral — not a data series)
    chart
        .draw_series(DashedLineSeries::new(
            [(0.0, r.target), (t_max, r.target)],
            8,
            5,
            SECONDARY_INK.stroke_width(1),
        ))?
        .label("cluster target")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 16, y)], SECONDARY_INK));

    // Event markers: vertical dashed lines with labels near the top
    for (t, label) in &r.events {
        chart.draw_series(DashedLineSeries::new(
            [(*t, 0.0), (*t, y_max)],
            4,
            4,
            MUTED.stroke_width(1),
        ))?;
        chart.draw_series(std::iter::once(Text::new(
            label.clone(),
            (*t, y_max * 0.97),
            ("sans-serif", 12).into_font().color(&SECONDARY_INK),
        )))?;
    }

    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::UpperRight)
        .background_style(SURFACE.mix(0.9))
        .border_style(AXIS)
        .label_font(("sans-serif", 13).into_font().color(&SECONDARY_INK))
        .draw()?;

    root.present()?;
    Ok(())
}
