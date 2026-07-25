//! Seeded, reproducible workload generation.
//!
//! A workload describes offered load for one scope: a deterministic rate
//! curve over simulated time (`LoadPattern`), how individual arrivals are
//! drawn from that rate (`ArrivalProcess`), and how the load is split across
//! nodes (`node_weights`).

use std::time::Duration;

/// A single sine component for [`LoadPattern::Sinusoidal`].
#[derive(Debug, Clone)]
pub struct SineComponent {
    pub amplitude: f64,
    pub frequency_hz: f64,
}

/// Offered load rate as a function of simulated time. All rates are
/// requests/second for the whole cluster; negative intermediate values are
/// clamped to zero.
#[derive(Debug, Clone)]
pub enum LoadPattern {
    /// Constant rate for the whole run
    Constant { rate: f64 },

    /// `before` until `at`, then `after`
    Step {
        before: f64,
        after: f64,
        at: Duration,
    },

    /// Linear interpolation from `from` to `to` over `[start, start + ramp]`,
    /// holding `to` afterwards
    Ramp {
        from: f64,
        to: f64,
        start: Duration,
        ramp: Duration,
    },

    /// `base`, with `spike` (absolute rate) for the first `spike_duration` of
    /// every `period`
    Burst {
        base: f64,
        spike: f64,
        period: Duration,
        spike_duration: Duration,
    },

    /// Base rate plus a sum of sine waves — ported from the retired
    /// `request_simulator_plot` example, where it was the manual tuning
    /// pattern for exercising PID tracking of smoothly varying load
    Sinusoidal {
        base: f64,
        components: Vec<SineComponent>,
    },

    /// Piecewise-constant: the rate of the last `(at, rate)` step whose
    /// time is ≤ t (0.0 before the first step). Steps must be sorted by
    /// time. Used for multi-phase journeys like tail → hot → tail.
    Piecewise { steps: Vec<(Duration, f64)> },
}

impl LoadPattern {
    /// The offered rate (requests/second, cluster-wide) at simulated time `t`.
    pub fn rate_at(&self, t: Duration) -> f64 {
        let secs = t.as_secs_f64();
        let rate = match self {
            LoadPattern::Constant { rate } => *rate,
            LoadPattern::Step { before, after, at } => {
                if t < *at {
                    *before
                } else {
                    *after
                }
            }
            LoadPattern::Ramp {
                from,
                to,
                start,
                ramp,
            } => {
                if t <= *start {
                    *from
                } else if t >= *start + *ramp {
                    *to
                } else {
                    let progress = (t - *start).as_secs_f64() / ramp.as_secs_f64();
                    from + (to - from) * progress
                }
            }
            LoadPattern::Burst {
                base,
                spike,
                period,
                spike_duration,
            } => {
                let into_period = secs % period.as_secs_f64();
                if into_period < spike_duration.as_secs_f64() {
                    *spike
                } else {
                    *base
                }
            }
            LoadPattern::Sinusoidal { base, components } => {
                let mut rate = *base;
                for c in components {
                    rate +=
                        c.amplitude * (2.0 * std::f64::consts::PI * c.frequency_hz * secs).sin();
                }
                rate
            }
            LoadPattern::Piecewise { steps } => steps
                .iter()
                .take_while(|(at, _)| *at <= t)
                .last()
                .map(|(_, rate)| *rate)
                .unwrap_or(0.0),
        };
        rate.max(0.0)
    }
}

/// How arrival counts are drawn from the rate curve each tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrivalProcess {
    /// Exactly `rate × dt` arrivals via a fractional accumulator — no
    /// sampling noise, useful for isolating control-loop dynamics
    Deterministic,

    /// Poisson-distributed arrivals with mean `rate × dt` — realistic
    /// request-arrival noise (seeded, reproducible)
    Poisson,
}

/// Offered load for one scope.
#[derive(Debug, Clone)]
pub struct Workload {
    /// Scope name (each node auto-creates a limiter per scope, as the server
    /// does)
    pub scope: String,

    /// Cluster-wide offered rate over time
    pub pattern: LoadPattern,

    /// Arrival sampling
    pub arrival: ArrivalProcess,

    /// Per-node share of the offered load. `None` = uniform across nodes
    /// that are up. Weights are normalized over up nodes each tick, so a
    /// down node's share is redistributed proportionally — matching a load
    /// balancer that stops routing to a dead instance.
    pub node_weights: Option<Vec<f64>>,
}

/// How a population's per-user traffic maps onto nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Routing {
    /// Each arrival lands on a uniformly random up node — the
    /// load-balancer assumption behind the `local_rate × num_nodes`
    /// promotion estimate
    Uniform,

    /// Session affinity: all of a user's traffic lands on one node
    /// (`user_rank % num_nodes`, skipping down nodes) — the worst case for
    /// the uniform-routing estimate
    Sticky,
}

/// A heavy-tailed population of per-user scopes (Milestone 6): `users`
/// distinct scopes named `{prefix}{rank}` sharing one total offered-rate
/// curve, split across users by a Zipf rank-frequency law
/// (`weight(rank) ∝ 1 / (rank + 1)^zipf_s`) — the discrete analog of
/// Pareto-distributed API usage. Arrivals are always Poisson.
#[derive(Debug, Clone)]
pub struct PopulationWorkload {
    /// Scope-name prefix (scope = `{prefix}{rank}`)
    pub prefix: String,

    /// Number of distinct users
    pub users: usize,

    /// Zipf exponent (1.0 ≈ classic rank-frequency; larger = heavier head)
    pub zipf_s: f64,

    /// Total offered rate over time (whole population, cluster-wide)
    pub pattern: LoadPattern,

    /// Per-arrival node routing
    pub routing: Routing,
}

impl PopulationWorkload {
    pub fn new(prefix: impl Into<String>, users: usize, zipf_s: f64, pattern: LoadPattern) -> Self {
        PopulationWorkload {
            prefix: prefix.into(),
            users,
            zipf_s,
            pattern,
            routing: Routing::Uniform,
        }
    }

    pub fn routing(mut self, routing: Routing) -> Self {
        self.routing = routing;
        self
    }
}

impl Workload {
    pub fn new(scope: impl Into<String>, pattern: LoadPattern) -> Self {
        Workload {
            scope: scope.into(),
            pattern,
            arrival: ArrivalProcess::Poisson,
            node_weights: None,
        }
    }

    pub fn arrival(mut self, arrival: ArrivalProcess) -> Self {
        self.arrival = arrival;
        self
    }

    pub fn node_weights(mut self, weights: Vec<f64>) -> Self {
        self.node_weights = Some(weights);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant() {
        let p = LoadPattern::Constant { rate: 100.0 };
        assert_eq!(p.rate_at(Duration::ZERO), 100.0);
        assert_eq!(p.rate_at(Duration::from_secs(1000)), 100.0);
    }

    #[test]
    fn test_step() {
        let p = LoadPattern::Step {
            before: 100.0,
            after: 200.0,
            at: Duration::from_secs(30),
        };
        assert_eq!(p.rate_at(Duration::from_secs(29)), 100.0);
        assert_eq!(p.rate_at(Duration::from_secs(30)), 200.0);
    }

    #[test]
    fn test_ramp() {
        let p = LoadPattern::Ramp {
            from: 0.0,
            to: 300.0,
            start: Duration::ZERO,
            ramp: Duration::from_secs(60),
        };
        assert_eq!(p.rate_at(Duration::ZERO), 0.0);
        assert!((p.rate_at(Duration::from_secs(30)) - 150.0).abs() < 1e-9);
        assert_eq!(p.rate_at(Duration::from_secs(90)), 300.0);
    }

    #[test]
    fn test_burst() {
        let p = LoadPattern::Burst {
            base: 100.0,
            spike: 1000.0,
            period: Duration::from_secs(10),
            spike_duration: Duration::from_secs(1),
        };
        assert_eq!(p.rate_at(Duration::from_millis(500)), 1000.0);
        assert_eq!(p.rate_at(Duration::from_secs(5)), 100.0);
        assert_eq!(p.rate_at(Duration::from_millis(10_500)), 1000.0);
    }

    #[test]
    fn test_sinusoidal_clamps_negative() {
        let p = LoadPattern::Sinusoidal {
            base: 10.0,
            components: vec![SineComponent {
                amplitude: 50.0,
                frequency_hz: 0.25,
            }],
        };
        // At t=3s the sine is at its trough: 10 - 50 < 0, clamped
        assert_eq!(p.rate_at(Duration::from_secs(3)), 0.0);
        // At t=1s the sine peaks: 10 + 50
        assert!((p.rate_at(Duration::from_secs(1)) - 60.0).abs() < 1e-9);
    }
}
