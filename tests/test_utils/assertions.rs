/// Statistical assertion helpers for time-series data validation
/// Assert that all values in a series are within a tolerance band of the target
///
/// # Arguments
/// * `series` - Time-series data to check
/// * `target` - Target value
/// * `tolerance_pct` - Tolerance as a percentage (e.g., 5.0 for ±5%)
/// * `message` - Custom error message
pub fn assert_within_tolerance(series: &[f64], target: f64, tolerance_pct: f64, message: &str) {
    let tolerance = target * (tolerance_pct / 100.0);
    let lower = target - tolerance;
    let upper = target + tolerance;

    for (i, &value) in series.iter().enumerate() {
        assert!(
            value >= lower && value <= upper,
            "{}: value[{}] = {} outside tolerance band [{}, {}] (target {} ± {}%)",
            message,
            i,
            value,
            lower,
            upper,
            target,
            tolerance_pct
        );
    }
}

/// Compute settling time - time taken to stay within tolerance band
///
/// Returns the time (index in series) when the value enters the tolerance band
/// and never leaves it afterward.
///
/// Returns None if the series never settles within the tolerance.
pub fn compute_settling_time(series: &[f64], target: f64, tolerance_pct: f64) -> Option<usize> {
    let tolerance = target * (tolerance_pct / 100.0);
    let lower = target - tolerance;
    let upper = target + tolerance;

    // Find the last time we left the tolerance band
    let last_exit = series
        .iter()
        .enumerate()
        .rev()
        .find(|(_, &value)| value < lower || value > upper)
        .map(|(i, _)| i);

    match last_exit {
        Some(exit_idx) => {
            // Settled at the point after the last exit
            if exit_idx + 1 < series.len() {
                Some(exit_idx + 1)
            } else {
                None // Never settled
            }
        }
        None => {
            // Never left the band, settled immediately if within band
            if !series.is_empty() && series[0] >= lower && series[0] <= upper {
                Some(0)
            } else {
                None
            }
        }
    }
}

/// Compute maximum overshoot beyond setpoint
///
/// Returns the maximum amount by which the series exceeded the setpoint.
/// Returns 0.0 if no overshoot occurred.
pub fn compute_max_overshoot(series: &[f64], setpoint: f64) -> f64 {
    series
        .iter()
        .map(|&value| (value - setpoint).max(0.0))
        .fold(0.0, f64::max)
}

/// Compute steady-state error after a given time
///
/// Returns the mean absolute error in the portion of the series after `after_time_idx`.
pub fn compute_steady_state_error(series: &[f64], target: f64, after_time_idx: usize) -> f64 {
    if after_time_idx >= series.len() {
        return 0.0;
    }

    let tail = &series[after_time_idx..];
    let sum_abs_error: f64 = tail.iter().map(|&v| (v - target).abs()).sum();
    sum_abs_error / tail.len() as f64
}

/// Detect oscillation by counting threshold crossings
///
/// Returns true if the series crosses the target more than `max_crossings` times.
pub fn detect_oscillation(series: &[f64], target: f64, max_crossings: usize) -> bool {
    if series.len() < 2 {
        return false;
    }

    let mut crossings = 0;
    let mut above = series[0] > target;

    for &value in series.iter().skip(1) {
        let now_above = value > target;
        if now_above != above {
            crossings += 1;
            above = now_above;
        }
    }

    crossings > max_crossings
}

/// Compute mean of a series
pub fn mean(series: &[f64]) -> f64 {
    if series.is_empty() {
        return 0.0;
    }
    series.iter().sum::<f64>() / series.len() as f64
}

/// Compute standard deviation of a series
pub fn stddev(series: &[f64]) -> f64 {
    if series.is_empty() {
        return 0.0;
    }

    let m = mean(series);
    let variance = series.iter().map(|v| (v - m).powi(2)).sum::<f64>() / series.len() as f64;
    variance.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assert_within_tolerance_pass() {
        let series = vec![95.0, 100.0, 105.0];
        assert_within_tolerance(&series, 100.0, 10.0, "should pass");
    }

    #[test]
    #[should_panic(expected = "outside tolerance band")]
    fn test_assert_within_tolerance_fail() {
        let series = vec![95.0, 100.0, 120.0]; // 120 is > 110 (100 + 10%)
        assert_within_tolerance(&series, 100.0, 10.0, "should fail");
    }

    #[test]
    fn test_compute_settling_time() {
        // Series that settles at index 3
        let series = vec![50.0, 80.0, 110.0, 95.0, 98.0, 100.0, 99.0];
        let settling_idx = compute_settling_time(&series, 100.0, 5.0);
        assert_eq!(settling_idx, Some(3));
    }

    #[test]
    fn test_compute_settling_time_never_settles() {
        let series = vec![50.0, 80.0, 110.0, 95.0, 110.0]; // Keeps oscillating
        let settling_idx = compute_settling_time(&series, 100.0, 5.0);
        assert_eq!(settling_idx, None);
    }

    #[test]
    fn test_compute_max_overshoot() {
        let series = vec![100.0, 110.0, 120.0, 105.0, 100.0];
        let overshoot = compute_max_overshoot(&series, 100.0);
        assert_eq!(overshoot, 20.0);
    }

    #[test]
    fn test_compute_max_overshoot_no_overshoot() {
        let series = vec![80.0, 90.0, 95.0, 98.0];
        let overshoot = compute_max_overshoot(&series, 100.0);
        assert_eq!(overshoot, 0.0);
    }

    #[test]
    fn test_detect_oscillation() {
        // Crosses target 4 times: up, down, up, down
        let series = vec![90.0, 110.0, 90.0, 110.0, 90.0];
        assert!(detect_oscillation(&series, 100.0, 2));
        assert!(!detect_oscillation(&series, 100.0, 10));
    }

    #[test]
    fn test_compute_steady_state_error() {
        let series = vec![50.0, 80.0, 95.0, 98.0, 99.0, 100.0, 101.0];
        // After index 4, error is: |99-100|=1, |100-100|=0, |101-100|=1
        // Mean = (1+0+1)/3 = 0.667
        let error = compute_steady_state_error(&series, 100.0, 4);
        assert!((error - 0.6666).abs() < 0.01);
    }

    #[test]
    fn test_mean_and_stddev() {
        let series = vec![2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let m = mean(&series);
        assert_eq!(m, 5.0);

        let sd = stddev(&series);
        assert!((sd - 2.0).abs() < 0.01);
    }
}
