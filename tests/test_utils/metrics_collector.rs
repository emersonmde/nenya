/// Time-series metrics collection for scenario tests
///
/// Single data point in a scenario test
#[derive(Debug, Clone)]
// Note: Some fields unused in current tests but part of complete metrics API for future scenarios
#[allow(dead_code)]
pub struct MetricRecord {
    pub time: f64,           // Simulated seconds from start
    pub generated_tps: f64,  // Input request rate
    pub accepted_tps: f64,   // Accepted rate
    pub throttled_tps: f64,  // Throttled rate
    pub target_rate: f64,    // PID output (adjusted target)
    pub pid_error: f64,      // Error signal (if available)
    pub pid_correction: f64, // PID correction value (if available)
}

/// Statistics computed over a time window
#[derive(Debug, Clone)]
pub struct WindowStats {
    pub mean: f64,
}

/// Collects time-series metrics during scenario tests
#[derive(Debug, Default)]
pub struct MetricsCollector {
    records: Vec<MetricRecord>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Record a single tick of data
    // Note: 8 parameters needed to capture complete state - represents single time-series sample
    #[allow(clippy::too_many_arguments)]
    pub fn record_tick(
        &mut self,
        time: f64,
        generated_tps: f64,
        accepted_tps: f64,
        throttled_tps: f64,
        target_rate: f64,
        pid_error: f64,
        pid_correction: f64,
    ) {
        self.records.push(MetricRecord {
            time,
            generated_tps,
            accepted_tps,
            throttled_tps,
            target_rate,
            pid_error,
            pid_correction,
        });
    }

    /// Get all records within a time window (inclusive)
    pub fn get_window(&self, start_time: f64, end_time: f64) -> Vec<&MetricRecord> {
        self.records
            .iter()
            .filter(|r| r.time >= start_time && r.time <= end_time)
            .collect()
    }

    /// Get all records
    pub fn all_records(&self) -> &[MetricRecord] {
        &self.records
    }

    /// Check if metrics collector is empty
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Get field values within a time window
    // Note: Helper method for extracting time-series data in tests
    #[allow(dead_code)]
    pub fn get_field<F>(&self, start_time: f64, end_time: f64, selector: F) -> Vec<f64>
    where
        F: Fn(&MetricRecord) -> f64,
    {
        self.get_window(start_time, end_time)
            .iter()
            .map(|r| selector(r))
            .collect()
    }

    /// Compute statistics over a time window using a field selector
    pub fn compute_stats<F>(
        &self,
        start_time: f64,
        end_time: f64,
        selector: F,
    ) -> Option<WindowStats>
    where
        F: Fn(&MetricRecord) -> f64,
    {
        let values: Vec<f64> = self
            .get_window(start_time, end_time)
            .iter()
            .map(|r| selector(r))
            .collect();

        if values.is_empty() {
            return None;
        }

        let mean = values.iter().sum::<f64>() / values.len() as f64;

        Some(WindowStats { mean })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_collector_basic() {
        let mut collector = MetricsCollector::new();
        assert!(collector.is_empty());

        collector.record_tick(0.0, 100.0, 80.0, 20.0, 100.0, 0.0, 0.0);
        collector.record_tick(0.1, 100.0, 85.0, 15.0, 100.0, 0.0, 0.0);

        assert_eq!(collector.all_records().len(), 2);
        assert!(!collector.is_empty());
    }

    #[test]
    fn test_get_window() {
        let mut collector = MetricsCollector::new();

        for i in 0..10 {
            let time = i as f64 * 0.1;
            collector.record_tick(time, 100.0, 80.0, 20.0, 100.0, 0.0, 0.0);
        }

        let window = collector.get_window(0.2, 0.5);
        assert_eq!(window.len(), 4); // 0.2, 0.3, 0.4, 0.5
    }

    #[test]
    fn test_compute_stats() {
        let mut collector = MetricsCollector::new();

        collector.record_tick(0.0, 100.0, 80.0, 20.0, 100.0, 0.0, 0.0);
        collector.record_tick(0.1, 100.0, 90.0, 10.0, 100.0, 0.0, 0.0);
        collector.record_tick(0.2, 100.0, 100.0, 0.0, 100.0, 0.0, 0.0);

        let stats = collector
            .compute_stats(0.0, 0.2, |r| r.accepted_tps)
            .expect("stats");

        assert_eq!(stats.mean, 90.0);
    }
}
