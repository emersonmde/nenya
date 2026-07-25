//! Per-run metrics, summary statistics, and artifact serialization.
//!
//! Output is plain CSV and JSON (hand-serialized — the `sim` feature adds no
//! dependencies) with fixed-precision formatting, so identical runs produce
//! byte-identical artifacts. That property is load-bearing: the determinism
//! guarantee is asserted by diffing these strings.

/// One sampled point of the run's time series.
#[derive(Debug, Clone)]
pub struct Sample {
    /// Simulated time in seconds (end of the sample window)
    pub t: f64,

    /// Offered load over the sample window (requests/second, cluster-wide)
    pub offered_rate: f64,

    /// Accepted rate over the sample window (requests/second, cluster-wide)
    pub accepted_rate: f64,

    /// Accepted rate per node over the sample window
    pub per_node_rate: Vec<f64>,
}

/// A convergence measurement anchored at an event (or run start).
#[derive(Debug, Clone)]
pub struct Convergence {
    /// Simulated time of the anchoring event, in seconds
    pub after: f64,

    /// Event label ("start", "node2_down", ...)
    pub label: String,

    /// Seconds from the event until the smoothed cluster rate entered the
    /// acceptance band and stayed there for the hold window; `None` if it
    /// never converged before the run ended (or the next event preempted it)
    pub time_to_converge: Option<f64>,
}

/// Summary metrics computed from the time series.
#[derive(Debug, Clone)]
pub struct Summary {
    /// Peak of (accepted − target), requests/second; 0 if never exceeded
    pub max_overshoot: f64,

    /// Time-integrated excess over target (requests): how many requests were
    /// admitted beyond what the target allowed
    pub integrated_overshoot: f64,

    /// Time-integrated shortfall below the achievable rate
    /// min(target, offered) (requests): throughput sacrificed
    pub integrated_undershoot: f64,

    /// Convergence time after run start and after each event
    pub convergence: Vec<Convergence>,

    /// Standard deviation of the cluster accepted rate over the steady
    /// window (final 25% of the run)
    pub steady_stddev: f64,

    /// Peak-to-peak spread of the cluster accepted rate over the steady window
    pub steady_peak_to_peak: f64,

    /// Coefficient of variation (stddev/mean) of per-node mean rates over
    /// the steady window, across nodes that carried traffic. Only meaningful
    /// under uniform load; `None` when fewer than two nodes carried traffic.
    pub fairness_cv: Option<f64>,
}

/// Complete result of one scenario run.
#[derive(Debug, Clone)]
pub struct RunResult {
    pub scenario: String,
    pub seed: u64,
    pub target: f64,
    pub num_nodes: usize,
    pub samples: Vec<Sample>,
    /// (time in seconds, label) of applied timeline events
    pub events: Vec<(f64, String)>,
    pub summary: Summary,
}

/// Acceptance band half-width as a fraction of the reference rate. The ±5%
/// band comes from the Milestone 4 acceptance criteria in the roadmap.
pub const CONVERGENCE_BAND: f64 = 0.05;

/// The smoothed rate must stay in band this long (seconds) to count as
/// converged. Chosen as 10 sync intervals at the production default (500ms):
/// long enough that a single lucky sample can't count, short relative to the
/// scenario durations used in the library.
pub const CONVERGENCE_HOLD_SECS: f64 = 5.0;

/// Trailing window (seconds) used to smooth the sampled rate before band
/// checks; spans multiple PID update intervals so Poisson sampling noise
/// doesn't flap the convergence detector.
pub const SMOOTHING_WINDOW_SECS: f64 = 2.0;

/// Compute summary metrics from a sampled time series.
///
/// `events` anchor convergence measurements; the measurement window for each
/// anchor ends at the next event (dynamics after that belong to the next
/// measurement).
pub fn summarize(
    samples: &[Sample],
    target: f64,
    events: &[(f64, String)],
    sample_interval_secs: f64,
) -> Summary {
    if samples.is_empty() {
        return Summary {
            max_overshoot: 0.0,
            integrated_overshoot: 0.0,
            integrated_undershoot: 0.0,
            convergence: Vec::new(),
            steady_stddev: 0.0,
            steady_peak_to_peak: 0.0,
            fairness_cv: None,
        };
    }

    let dt = sample_interval_secs;

    let mut max_overshoot: f64 = 0.0;
    let mut integrated_overshoot = 0.0;
    let mut integrated_undershoot = 0.0;
    for s in samples {
        let excess = s.accepted_rate - target;
        if excess > max_overshoot {
            max_overshoot = excess;
        }
        if excess > 0.0 {
            integrated_overshoot += excess * dt;
        }
        let achievable = target.min(s.offered_rate);
        let shortfall = achievable - s.accepted_rate;
        if shortfall > 0.0 {
            integrated_undershoot += shortfall * dt;
        }
    }

    // Smoothed series: trailing mean over SMOOTHING_WINDOW_SECS. The offered
    // series is smoothed identically — the band reference derives from it,
    // and comparing a smoothed signal against an unsmoothed reference would
    // reintroduce the sampling noise the smoothing removes.
    let window = (SMOOTHING_WINDOW_SECS / dt).round().max(1.0) as usize;
    let trailing_mean = |f: &dyn Fn(&Sample) -> f64| -> Vec<f64> {
        (0..samples.len())
            .map(|i| {
                let lo = i.saturating_sub(window - 1);
                let slice = &samples[lo..=i];
                slice.iter().map(f).sum::<f64>() / slice.len() as f64
            })
            .collect()
    };
    let smoothed = trailing_mean(&|s: &Sample| s.accepted_rate);
    let smoothed_offered = trailing_mean(&|s: &Sample| s.offered_rate);

    // Convergence after run start and after each event
    let mut anchors: Vec<(f64, String)> = Vec::with_capacity(events.len() + 1);
    anchors.push((0.0, "start".to_string()));
    anchors.extend(events.iter().cloned());

    let run_end = samples.last().map(|s| s.t).unwrap_or(0.0);
    let hold_samples = (CONVERGENCE_HOLD_SECS / dt).round().max(1.0) as usize;

    let mut convergence = Vec::new();
    for (idx, (anchor_t, label)) in anchors.iter().enumerate() {
        let window_end = anchors
            .get(idx + 1)
            .map(|(t, _)| *t)
            .unwrap_or(f64::INFINITY)
            .min(run_end);

        let mut time_to_converge = None;
        let mut in_band_since: Option<usize> = None;
        for (i, s) in samples.iter().enumerate() {
            if s.t < *anchor_t || s.t > window_end {
                continue;
            }
            // Reference: the target when demand can reach it, otherwise the
            // offered load (a cluster can't accept more than is offered).
            // The 1 rps floor keeps the band meaningful near zero load.
            let reference = target.min(smoothed_offered[i].max(1.0));
            let tolerance = (reference * CONVERGENCE_BAND).max(1.0);
            if (smoothed[i] - reference).abs() <= tolerance {
                let since = *in_band_since.get_or_insert(i);
                if i - since + 1 >= hold_samples {
                    time_to_converge = Some(samples[since].t - anchor_t);
                    break;
                }
            } else {
                in_band_since = None;
            }
        }
        convergence.push(Convergence {
            after: *anchor_t,
            label: label.clone(),
            time_to_converge,
        });
    }

    // Steady-state window: final 25% of the run
    let steady_start = samples.len() - (samples.len() / 4).max(1);
    let steady = &samples[steady_start..];
    let rates: Vec<f64> = steady.iter().map(|s| s.accepted_rate).collect();
    let (steady_stddev, _mean) = stddev_mean(&rates);
    let steady_peak_to_peak = rates.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
        - rates.iter().cloned().fold(f64::INFINITY, f64::min);

    // Fairness: CV of per-node mean rates across nodes that carried traffic
    let num_nodes = samples[0].per_node_rate.len();
    let node_means: Vec<f64> = (0..num_nodes)
        .map(|n| steady.iter().map(|s| s.per_node_rate[n]).sum::<f64>() / steady.len() as f64)
        .filter(|&m| m > 0.0)
        .collect();
    let fairness_cv = if node_means.len() >= 2 {
        let (sd, mean) = stddev_mean(&node_means);
        if mean > 0.0 {
            Some(sd / mean)
        } else {
            None
        }
    } else {
        None
    };

    Summary {
        max_overshoot,
        integrated_overshoot,
        integrated_undershoot,
        convergence,
        steady_stddev,
        steady_peak_to_peak: if steady_peak_to_peak.is_finite() {
            steady_peak_to_peak
        } else {
            0.0
        },
        fairness_cv,
    }
}

fn stddev_mean(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
    (variance.sqrt(), mean)
}

impl RunResult {
    /// Time-series CSV: `t,offered,accepted,node0,node1,...`
    pub fn to_csv(&self) -> String {
        let mut out = String::from("t,offered_rate,accepted_rate");
        for i in 0..self.num_nodes {
            out.push_str(&format!(",node{}", i));
        }
        out.push('\n');
        for s in &self.samples {
            out.push_str(&format!(
                "{:.3},{:.4},{:.4}",
                s.t, s.offered_rate, s.accepted_rate
            ));
            for r in &s.per_node_rate {
                out.push_str(&format!(",{:.4}", r));
            }
            out.push('\n');
        }
        out
    }

    /// Full result as JSON (samples, events, summary).
    pub fn to_json(&self) -> String {
        let mut out = String::from("{\n");
        out.push_str(&format!("  \"scenario\": {},\n", json_str(&self.scenario)));
        out.push_str(&format!("  \"seed\": {},\n", self.seed));
        out.push_str(&format!("  \"target\": {:.4},\n", self.target));
        out.push_str(&format!("  \"num_nodes\": {},\n", self.num_nodes));

        out.push_str("  \"events\": [");
        for (i, (t, label)) in self.events.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&format!(
                "{{\"t\": {:.3}, \"label\": {}}}",
                t,
                json_str(label)
            ));
        }
        out.push_str("],\n");

        let s = &self.summary;
        out.push_str("  \"summary\": {\n");
        out.push_str(&format!("    \"max_overshoot\": {:.4},\n", s.max_overshoot));
        out.push_str(&format!(
            "    \"integrated_overshoot\": {:.4},\n",
            s.integrated_overshoot
        ));
        out.push_str(&format!(
            "    \"integrated_undershoot\": {:.4},\n",
            s.integrated_undershoot
        ));
        out.push_str(&format!("    \"steady_stddev\": {:.4},\n", s.steady_stddev));
        out.push_str(&format!(
            "    \"steady_peak_to_peak\": {:.4},\n",
            s.steady_peak_to_peak
        ));
        match s.fairness_cv {
            Some(cv) => out.push_str(&format!("    \"fairness_cv\": {:.4},\n", cv)),
            None => out.push_str("    \"fairness_cv\": null,\n"),
        }
        out.push_str("    \"convergence\": [");
        for (i, c) in s.convergence.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let t = match c.time_to_converge {
                Some(v) => format!("{:.3}", v),
                None => "null".to_string(),
            };
            out.push_str(&format!(
                "{{\"after\": {:.3}, \"label\": {}, \"time_to_converge\": {}}}",
                c.after,
                json_str(&c.label),
                t
            ));
        }
        out.push_str("]\n  },\n");

        out.push_str("  \"samples\": [\n");
        for (i, sample) in self.samples.iter().enumerate() {
            out.push_str(&format!(
                "    {{\"t\": {:.3}, \"offered\": {:.4}, \"accepted\": {:.4}, \"per_node\": [",
                sample.t, sample.offered_rate, sample.accepted_rate
            ));
            for (j, r) in sample.per_node_rate.iter().enumerate() {
                if j > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("{:.4}", r));
            }
            out.push(']');
            out.push('}');
            if i + 1 < self.samples.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("  ]\n}\n");
        out
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_samples(rate: f64, n: usize, dt: f64) -> Vec<Sample> {
        (0..n)
            .map(|i| Sample {
                t: (i + 1) as f64 * dt,
                offered_rate: rate * 2.0,
                accepted_rate: rate,
                per_node_rate: vec![rate / 2.0, rate / 2.0],
            })
            .collect()
    }

    #[test]
    fn test_summary_flat_at_target() {
        let samples = flat_samples(300.0, 600, 0.1);
        let summary = summarize(&samples, 300.0, &[], 0.1);
        assert_eq!(summary.max_overshoot, 0.0);
        assert_eq!(summary.integrated_overshoot, 0.0);
        assert!(summary.integrated_undershoot < 1e-9);
        assert_eq!(summary.steady_stddev, 0.0);
        assert_eq!(summary.convergence.len(), 1);
        // Flat at target: converged as soon as the hold window fills
        let t = summary.convergence[0].time_to_converge.unwrap();
        assert!(
            t < 1.0,
            "flat series should converge immediately, got {}",
            t
        );
        // Perfectly fair
        assert!(summary.fairness_cv.unwrap() < 1e-12);
    }

    #[test]
    fn test_summary_overshoot_accounting() {
        let mut samples = flat_samples(300.0, 600, 0.1);
        // 10 samples (1 simulated second) at 400 rps: 100 rps excess × 1s
        for s in samples.iter_mut().take(10) {
            s.accepted_rate = 400.0;
        }
        let summary = summarize(&samples, 300.0, &[], 0.1);
        assert_eq!(summary.max_overshoot, 100.0);
        assert!((summary.integrated_overshoot - 100.0).abs() < 1e-6);
    }

    #[test]
    fn test_never_converges() {
        let samples = flat_samples(500.0, 300, 0.1); // 500 vs target 300
        let summary = summarize(&samples, 300.0, &[], 0.1);
        assert!(summary.convergence[0].time_to_converge.is_none());
    }

    #[test]
    fn test_csv_and_json_shape() {
        let samples = flat_samples(300.0, 5, 0.1);
        let summary = summarize(&samples, 300.0, &[], 0.1);
        let result = RunResult {
            scenario: "test".to_string(),
            seed: 42,
            target: 300.0,
            num_nodes: 2,
            samples,
            events: vec![(0.2, "node1_down".to_string())],
            summary,
        };
        let csv = result.to_csv();
        assert!(csv.starts_with("t,offered_rate,accepted_rate,node0,node1\n"));
        assert_eq!(csv.lines().count(), 6);

        let json = result.to_json();
        assert!(json.contains("\"scenario\": \"test\""));
        assert!(json.contains("\"node1_down\""));
    }
}
