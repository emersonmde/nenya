/// Request generators for simulating various load patterns
use std::f64::consts::PI;

/// Trait for request generators that produce time-varying load patterns
pub trait RequestGenerator {
    /// Returns the target TPS (transactions per second) at the given time
    fn tps_at_time(&self, time_secs: f64) -> f64;
}

/// Constant load generator - produces steady TPS
#[derive(Debug, Clone)]
pub struct ConstantLoadGenerator {
    tps: f64,
}

impl ConstantLoadGenerator {
    pub fn new(tps: f64) -> Self {
        Self { tps }
    }
}

impl RequestGenerator for ConstantLoadGenerator {
    fn tps_at_time(&self, _time_secs: f64) -> f64 {
        self.tps
    }
}

/// Sinusoidal load generator - produces oscillating load pattern
#[derive(Debug, Clone)]
// Note: Used for oscillating load pattern tests
#[allow(dead_code)]
pub struct SinusoidalLoadGenerator {
    baseline: f64,
    amplitude: f64,
    period_secs: f64,
}

impl SinusoidalLoadGenerator {
    #[allow(dead_code)]
    pub fn new(baseline: f64, amplitude: f64, period_secs: f64) -> Self {
        Self {
            baseline,
            amplitude,
            period_secs,
        }
    }
}

impl RequestGenerator for SinusoidalLoadGenerator {
    fn tps_at_time(&self, time_secs: f64) -> f64 {
        let omega = 2.0 * PI / self.period_secs;
        self.baseline + self.amplitude * (omega * time_secs).sin()
    }
}

/// Step load generator - switches between two levels at a specific time
#[derive(Debug, Clone)]
// Note: Used in scenario tests for step response analysis
#[allow(dead_code)]
pub struct StepLoadGenerator {
    initial_tps: f64,
    final_tps: f64,
    step_time: f64,
}

impl StepLoadGenerator {
    #[allow(dead_code)]
    pub fn new(initial_tps: f64, final_tps: f64, step_time: f64) -> Self {
        Self {
            initial_tps,
            final_tps,
            step_time,
        }
    }
}

impl RequestGenerator for StepLoadGenerator {
    fn tps_at_time(&self, time_secs: f64) -> f64 {
        if time_secs < self.step_time {
            self.initial_tps
        } else {
            self.final_tps
        }
    }
}

/// Linear ramp load generator - linearly increases/decreases TPS over time
#[derive(Debug, Clone)]
// Note: Used in scenario tests for ramp-up response analysis
#[allow(dead_code)]
pub struct RampLoadGenerator {
    start_tps: f64,
    end_tps: f64,
    duration_secs: f64,
}

impl RampLoadGenerator {
    #[allow(dead_code)]
    pub fn new(start_tps: f64, end_tps: f64, duration_secs: f64) -> Self {
        Self {
            start_tps,
            end_tps,
            duration_secs,
        }
    }
}

impl RequestGenerator for RampLoadGenerator {
    fn tps_at_time(&self, time_secs: f64) -> f64 {
        if time_secs >= self.duration_secs {
            return self.end_tps;
        }

        let progress = time_secs / self.duration_secs;
        self.start_tps + (self.end_tps - self.start_tps) * progress
    }
}

/// Burst load generator - alternates between normal and burst load
#[derive(Debug, Clone)]
// Note: Used in scenario tests for burst handling and overload analysis
#[allow(dead_code)]
pub struct BurstLoadGenerator {
    normal_tps: f64,
    burst_tps: f64,
    burst_start: f64,
    burst_duration: f64,
}

impl BurstLoadGenerator {
    #[allow(dead_code)]
    pub fn new(normal_tps: f64, burst_tps: f64, burst_start: f64, burst_duration: f64) -> Self {
        Self {
            normal_tps,
            burst_tps,
            burst_start,
            burst_duration,
        }
    }
}

impl RequestGenerator for BurstLoadGenerator {
    fn tps_at_time(&self, time_secs: f64) -> f64 {
        let burst_end = self.burst_start + self.burst_duration;
        if time_secs >= self.burst_start && time_secs < burst_end {
            self.burst_tps
        } else {
            self.normal_tps
        }
    }
}
