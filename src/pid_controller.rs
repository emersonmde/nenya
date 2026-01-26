/// A PID controller for managing control loops.
///
/// This controller allows for proportional, integral, and derivative (PID) control, which can be
/// used to maintain a setpoint in a dynamic system. The controller computes a correction based on
/// the difference between a desired setpoint and a measured process variable.
///
/// # Example
///
/// ```rust
/// use nenya::pid_controller::PIDControllerBuilder;
///
/// let mut pid_controller = PIDControllerBuilder::new(10.0)
///     .kp(1.0)
///     .ki(0.1)
///     .kd(0.01)
///     .build();
///
/// let correction: f32 = pid_controller.compute_correction(8.0);
/// println!("Correction: {}", correction);
/// ```
use num_traits::{Float, Signed};

#[derive(Debug, Clone)]
pub struct PIDController<T> {
    setpoint: T,
    kp: T,
    ki: T,
    kd: T,
    error_bias: T,
    error_limit: Option<T>,
    output_limit: Option<T>,
    accumulated_error: T,
    previous_error: T,
}

impl<T: Float + Signed + Copy> PIDController<T> {
    /// Creates a new `PIDController`.
    ///
    /// This method initializes the PID controller with specified parameters, including gains for
    /// the proportional (`kp`), integral (`ki`), and derivative (`kd`) components, as well as an
    /// error bias, and optional limits for the error and output.
    pub fn new(
        setpoint: T,
        kp: T,
        ki: T,
        kd: T,
        error_bias: T,
        error_limit: Option<T>,
        output_limit: Option<T>,
    ) -> Self {
        PIDController {
            setpoint,
            kp,
            ki,
            kd,
            error_limit,
            output_limit,
            accumulated_error: T::zero(),
            previous_error: T::zero(),
            error_bias,
        }
    }

    /// Creates a new static `PIDController` with zero gains.
    ///
    /// This method is useful for scenarios where a static controller with no dynamic adjustments is
    /// needed. The error bias is set to one.
    pub fn new_static_controller(setpoint: T) -> Self {
        PIDController {
            setpoint,
            kp: T::zero(),
            ki: T::zero(),
            kd: T::zero(),
            error_limit: None,
            output_limit: None,
            accumulated_error: T::zero(),
            previous_error: T::zero(),
            error_bias: T::one(),
        }
    }

    /// Computes the correction based on the current error.
    ///
    /// This method calculates the PID correction using the proportional, integral, and derivative
    /// components. The computed correction is clamped if the output limit is set, and anti-windup
    /// feedback correction is applied if necessary.
    pub fn compute_correction(&mut self, signal: impl Into<T>) -> T {
        let error = self.setpoint - signal.into();
        let p = self.kp * error;

        // Apply error bias
        let biased_error = if error.is_positive() {
            error * (num_traits::one::<T>() + self.error_bias)
        } else {
            error * (num_traits::one::<T>() - self.error_bias)
        };
        self.accumulated_error = self.accumulated_error + biased_error;

        // Clamp accumulated_error to prevent integral windup
        if let Some(error_limit) = self.error_limit {
            self.accumulated_error = num_traits::clamp(
                self.accumulated_error,
                -error_limit.abs(),
                error_limit.abs(),
            );
        }

        let i = self.ki * self.accumulated_error;
        let d = self.kd * (error - self.previous_error);

        let correction = p + i + d;
        let clamped_correction = if let Some(output_limit) = self.output_limit {
            num_traits::clamp(correction, -output_limit.abs(), output_limit.abs())
        } else {
            correction
        };

        // Anti-windup feedback correction
        if correction != clamped_correction {
            let feedback = correction - clamped_correction;
            self.accumulated_error = self.accumulated_error - (feedback / self.ki);
        }

        self.previous_error = error;

        clamped_correction
    }

    /// Returns the accumulated error of the PID controller.
    pub fn accumulated_error(&self) -> T {
        self.accumulated_error
    }

    /// Returns the setpoint of the PID controller.
    pub fn setpoint(&self) -> T {
        self.setpoint
    }

    /// Sets a new setpoint for the PID controller.
    ///
    /// This allows dynamic adjustment of the target value, useful for distributed
    /// coordination where the setpoint changes based on cluster size.
    pub fn set_setpoint(&mut self, setpoint: T) {
        self.setpoint = setpoint;
    }
}

/// Builder for creating a `PIDController` instance.
pub struct PIDControllerBuilder<T> {
    setpoint: T,
    kp: T,
    ki: T,
    kd: T,
    error_bias: T,
    error_limit: Option<T>,
    output_limit: Option<T>,
}

impl<T: Float + Signed + Copy> PIDControllerBuilder<T> {
    /// Creates a new `PIDControllerBuilder` with default values.
    pub fn new(setpoint: impl Into<T>) -> Self {
        PIDControllerBuilder {
            setpoint: setpoint.into(),
            kp: T::zero(),
            ki: T::zero(),
            kd: T::zero(),
            error_bias: T::one(),
            error_limit: None,
            output_limit: None,
        }
    }

    /// Sets the proportional gain (`kp`).
    pub fn kp(mut self, kp: impl Into<T>) -> Self {
        self.kp = kp.into();
        self
    }

    /// Sets the integral gain (`ki`).
    pub fn ki(mut self, ki: impl Into<T>) -> Self {
        self.ki = ki.into();
        self
    }

    /// Sets the derivative gain (`kd`).
    pub fn kd(mut self, kd: impl Into<T>) -> Self {
        self.kd = kd.into();
        self
    }

    /// Sets the error bias.
    pub fn error_bias(mut self, error_bias: impl Into<T>) -> Self {
        self.error_bias = error_bias.into();
        self
    }

    /// Sets the error limit.
    pub fn error_limit(mut self, error_limit: impl Into<T>) -> Self {
        self.error_limit = Some(error_limit.into());
        self
    }

    /// Sets the output limit.
    pub fn output_limit(mut self, output_limit: impl Into<T>) -> Self {
        self.output_limit = Some(output_limit.into());
        self
    }

    /// Builds and returns the `PIDController` instance.
    pub fn build(self) -> PIDController<T> {
        PIDController {
            setpoint: self.setpoint,
            kp: self.kp,
            ki: self.ki,
            kd: self.kd,
            error_bias: self.error_bias,
            error_limit: self.error_limit,
            output_limit: self.output_limit,
            accumulated_error: T::zero(),
            previous_error: T::zero(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Utility function to create a PIDController with defaults
    fn create_pid_controller<T: Float + Signed + Copy>(
        setpoint: T,
        kp: T,
        ki: T,
        kd: T,
        error_bias: T,
        error_limit: Option<T>,
        output_limit: Option<T>,
    ) -> PIDController<T> {
        let mut pid_controller_builder = PIDControllerBuilder::new(setpoint)
            .kp(kp)
            .ki(ki)
            .kd(kd)
            .error_bias(error_bias);

        if let Some(error_limit) = error_limit {
            pid_controller_builder = pid_controller_builder.error_limit(error_limit);
        }

        if let Some(output_limit) = output_limit {
            pid_controller_builder = pid_controller_builder.output_limit(output_limit);
        }

        pid_controller_builder.build()
    }

    #[test]
    fn test_pid_initialization() {
        let pid = create_pid_controller(1.0, 2.0, 3.0, 4.0, 0.5, Some(10.0), Some(5.0));
        assert_eq!(pid.setpoint, 1.0);
        assert_eq!(pid.kp, 2.0);
        assert_eq!(pid.ki, 3.0);
        assert_eq!(pid.kd, 4.0);
        assert_eq!(pid.error_bias, 0.5);
        assert_eq!(pid.error_limit, Some(10.0));
        assert_eq!(pid.output_limit, Some(5.0));
        assert_eq!(pid.accumulated_error, 0.0);
        assert_eq!(pid.previous_error, 0.0);
    }

    #[test]
    fn test_pid_compute_correction() {
        let mut pid = create_pid_controller(1.0, 2.0, 3.0, 4.0, 0.5, None, None);
        let correction = pid.compute_correction(0.5);
        assert!(correction > 0.0);
    }

    #[test]
    fn test_pid_compute_correction_with_error_limit() {
        let mut pid = create_pid_controller(1.0, 2.0, 3.0, 4.0, 0.5, Some(0.1), None);
        let correction = pid.compute_correction(0.5);
        assert!(correction > 0.0);
        assert!(pid.accumulated_error <= 0.1);
    }

    #[test]
    fn test_pid_compute_correction_with_output_limit() {
        let mut pid = create_pid_controller(1.0, 2.0, 3.0, 4.0, 0.5, None, Some(0.1));
        let correction = pid.compute_correction(0.5);
        assert!(correction <= 0.1);
    }

    #[test]
    fn test_pid_zero_gains() {
        let mut pid = create_pid_controller(1.0, 0.0, 0.0, 0.0, 0.0, None, None);
        let correction = pid.compute_correction(0.5);
        assert_eq!(correction, 0.0);
    }

    #[test]
    fn test_pid_negative_feedback() {
        let mut pid = create_pid_controller(1.0, -2.0, -3.0, -4.0, 0.5, None, None);
        let correction = pid.compute_correction(0.5);
        assert!(correction < 0.0);
    }

    #[test]
    fn test_pid_anti_windup() {
        let mut pid = create_pid_controller(1.0, 2.0, 3.0, 4.0, 0.5, Some(0.1), Some(0.5));
        pid.compute_correction(0.5);
        let correction = pid.compute_correction(0.5);
        assert!(correction <= 0.5);
    }

    #[test]
    fn test_pid_accumulated_error() {
        let mut pid = create_pid_controller(1.0, 2.0, 3.0, 4.0, 0.5, None, None);
        pid.compute_correction(0.5);
        assert!(pid.accumulated_error() > 0.0);
    }

    #[test]
    fn test_pid_setpoint() {
        let pid = create_pid_controller(1.0, 2.0, 3.0, 4.0, 0.5, None, None);
        assert_eq!(pid.setpoint, 1.0);
    }

    // ===== Enhanced Unit Tests for Individual PID Components =====

    #[test]
    fn test_error_calculation() {
        // Error should be: setpoint - process_variable
        let pid = create_pid_controller(100.0, 1.0, 0.0, 0.0, 0.0, None, None);

        // Test positive error (process_var < setpoint)
        let mut pid_pos = pid.clone();
        let correction_pos = pid_pos.compute_correction(80.0);
        assert!(
            correction_pos > 0.0,
            "Positive error should give positive correction"
        );

        // Test negative error (process_var > setpoint)
        let mut pid_neg = pid.clone();
        let correction_neg = pid_neg.compute_correction(120.0);
        assert!(
            correction_neg < 0.0,
            "Negative error should give negative correction"
        );

        // Test zero error
        let mut pid_zero = pid.clone();
        let correction_zero = pid_zero.compute_correction(100.0);
        assert_eq!(
            correction_zero, 0.0,
            "Zero error should give zero correction"
        );
    }

    #[test]
    fn test_proportional_term_only() {
        // With ki=0, kd=0, correction should be exactly kp * error
        let mut pid = create_pid_controller(100.0, 2.0, 0.0, 0.0, 0.0, None, None);

        let correction = pid.compute_correction(80.0);
        // error = 100 - 80 = 20
        // P = kp * error = 2.0 * 20 = 40
        assert_eq!(correction, 40.0);

        let correction2 = pid.compute_correction(110.0);
        // error = 100 - 110 = -10
        // P = 2.0 * -10 = -20
        assert_eq!(correction2, -20.0);
    }

    #[test]
    fn test_error_bias_positive() {
        // Positive error_bias should amplify positive errors and reduce negative errors
        let error_bias = 0.5; // 50% bias

        let mut pid = create_pid_controller(100.0, 1.0, 1.0, 0.0, error_bias, None, None);

        // Positive error: 100 - 80 = 20
        // Biased error for integral: 20 * (1 + 0.5) = 30
        pid.compute_correction(80.0);
        let accumulated1 = pid.accumulated_error();
        assert!(
            (accumulated1 - 30.0).abs() < 0.001,
            "Positive error should be amplified by bias"
        );

        // Negative error: 100 - 120 = -20
        // Biased error: -20 * (1 - 0.5) = -10
        let mut pid2 = create_pid_controller(100.0, 1.0, 1.0, 0.0, error_bias, None, None);
        pid2.compute_correction(120.0);
        let accumulated2 = pid2.accumulated_error();
        assert!(
            (accumulated2 - (-10.0)).abs() < 0.001,
            "Negative error should be reduced by bias"
        );
    }

    #[test]
    fn test_error_bias_negative() {
        // Negative error_bias should reduce positive errors and amplify negative errors
        let error_bias = -0.5;

        let mut pid = create_pid_controller(100.0, 1.0, 1.0, 0.0, error_bias, None, None);

        // Positive error: 100 - 80 = 20
        // Biased error: 20 * (1 - 0.5) = 10
        pid.compute_correction(80.0);
        let accumulated1 = pid.accumulated_error();
        assert!(
            (accumulated1 - 10.0).abs() < 0.001,
            "Positive error should be reduced by negative bias"
        );

        // Negative error: 100 - 120 = -20
        // Biased error: -20 * (1 + 0.5) = -30
        let mut pid2 = create_pid_controller(100.0, 1.0, 1.0, 0.0, error_bias, None, None);
        pid2.compute_correction(120.0);
        let accumulated2 = pid2.accumulated_error();
        assert!(
            (accumulated2 - (-30.0)).abs() < 0.001,
            "Negative error should be amplified by negative bias"
        );
    }

    #[test]
    fn test_integral_term_only() {
        // With kp=0, kd=0, correction should be ki * accumulated_error
        let mut pid = create_pid_controller(100.0, 0.0, 2.0, 0.0, 0.0, None, None);

        // First call: error = 20, accumulated = 20, I = 2.0 * 20 = 40
        let correction1 = pid.compute_correction(80.0);
        assert_eq!(correction1, 40.0);

        // Second call: error = 20, accumulated = 40, I = 2.0 * 40 = 80
        let correction2 = pid.compute_correction(80.0);
        assert_eq!(correction2, 80.0);

        // Third call: error = 20, accumulated = 60, I = 2.0 * 60 = 120
        let correction3 = pid.compute_correction(80.0);
        assert_eq!(correction3, 120.0);
    }

    #[test]
    fn test_integral_error_limit_clamping() {
        // accumulated_error should never exceed error_limit
        let error_limit = 10.0;
        let mut pid = create_pid_controller(100.0, 0.0, 1.0, 0.0, 0.0, Some(error_limit), None);

        // Generate large errors to try to exceed limit
        for _ in 0..20 {
            pid.compute_correction(0.0); // error = 100
        }

        let accumulated = pid.accumulated_error();
        assert!(
            accumulated.abs() <= error_limit,
            "accumulated_error {} exceeds limit {}",
            accumulated,
            error_limit
        );

        // Try negative errors
        let mut pid2 = create_pid_controller(100.0, 0.0, 1.0, 0.0, 0.0, Some(error_limit), None);
        for _ in 0..20 {
            pid2.compute_correction(200.0); // error = -100
        }

        let accumulated2 = pid2.accumulated_error();
        assert!(
            accumulated2.abs() <= error_limit,
            "accumulated_error {} exceeds limit {}",
            accumulated2,
            error_limit
        );
    }

    #[test]
    fn test_derivative_term_only() {
        // With kp=0, ki=0, correction should be kd * (error - previous_error)
        let mut pid = create_pid_controller(100.0, 0.0, 0.0, 3.0, 0.0, None, None);

        // First call: previous_error = 0, current_error = 20
        // D = 3.0 * (20 - 0) = 60
        let correction1 = pid.compute_correction(80.0);
        assert_eq!(correction1, 60.0);

        // Second call: previous_error = 20, current_error = 10
        // D = 3.0 * (10 - 20) = -30
        let correction2 = pid.compute_correction(90.0);
        assert_eq!(correction2, -30.0);

        // Third call: previous_error = 10, current_error = 10 (no change)
        // D = 3.0 * (10 - 10) = 0
        let correction3 = pid.compute_correction(90.0);
        assert_eq!(correction3, 0.0);
    }

    #[test]
    fn test_raw_correction_sum() {
        // Verify that correction = P + I + D (before clamping)
        let mut pid = create_pid_controller(100.0, 1.0, 0.5, 0.2, 0.0, None, None);

        // First call: error = 20
        // P = 1.0 * 20 = 20
        // I = 0.5 * 20 = 10
        // D = 0.2 * (20 - 0) = 4
        // Total = 20 + 10 + 4 = 34
        let correction = pid.compute_correction(80.0);
        assert_eq!(correction, 34.0);
    }

    #[test]
    fn test_output_clamping_positive() {
        // Force large positive correction, verify clamps to +output_limit
        let output_limit = 10.0;
        let mut pid = create_pid_controller(100.0, 10.0, 0.0, 0.0, 0.0, None, Some(output_limit));

        // error = 100 - 0 = 100
        // P = 10.0 * 100 = 1000 (huge)
        // Should clamp to +10.0
        let correction = pid.compute_correction(0.0);
        assert_eq!(correction, output_limit);
    }

    #[test]
    fn test_output_clamping_negative() {
        // Force large negative correction, verify clamps to -output_limit
        let output_limit = 10.0;
        let mut pid = create_pid_controller(100.0, 10.0, 0.0, 0.0, 0.0, None, Some(output_limit));

        // error = 100 - 200 = -100
        // P = 10.0 * -100 = -1000 (huge negative)
        // Should clamp to -10.0
        let correction = pid.compute_correction(200.0);
        assert_eq!(correction, -output_limit);
    }

    #[test]
    fn test_anti_windup_feedback_mechanism() {
        // When output is clamped, accumulated_error should be reduced
        let output_limit = 10.0;
        let mut pid = create_pid_controller(100.0, 0.0, 2.0, 0.0, 0.0, None, Some(output_limit));

        // First call: error = 100, I = 2.0 * 100 = 200, clamps to 10.0
        // Clamped amount = 200 - 10 = 190
        // Anti-windup reduces accumulated_error by 190 / 2.0 = 95
        // New accumulated_error = 100 - 95 = 5
        pid.compute_correction(0.0);
        let accumulated = pid.accumulated_error();

        assert!(
            accumulated.abs() < 100.0,
            "Anti-windup should reduce accumulated error, got {}",
            accumulated
        );

        // Verify accumulated_error was reduced (should be much less than 100)
        assert!(
            accumulated < 10.0,
            "Anti-windup should significantly reduce accumulated error"
        );
    }

    #[test]
    fn test_full_pid_loop_with_all_terms() {
        // All gains non-zero, error bias set, limits set
        // Verify all components contribute correctly
        let mut pid = create_pid_controller(100.0, 1.0, 0.5, 0.3, 0.2, Some(50.0), Some(20.0));

        let mut corrections = Vec::new();

        // Run multiple iterations with varying errors
        let process_vars = vec![80.0, 85.0, 90.0, 95.0, 98.0, 100.0, 102.0, 100.0, 98.0];

        for &pv in &process_vars {
            let correction = pid.compute_correction(pv);
            corrections.push(correction);

            // Output should always be bounded
            assert!(
                correction.abs() <= 20.0,
                "Correction {} exceeds output limit",
                correction
            );
        }

        // Verify accumulated error is bounded by error_limit
        assert!(
            pid.accumulated_error().abs() <= 50.0,
            "Accumulated error {} exceeds limit",
            pid.accumulated_error()
        );

        // Verify we got meaningful corrections (not all zeros)
        assert!(
            corrections.iter().any(|&c| c.abs() > 0.1),
            "PID should produce non-zero corrections"
        );
    }

    #[test]
    fn test_pid_state_persistence() {
        // Verify that accumulated_error and previous_error persist across calls
        let mut pid = create_pid_controller(100.0, 0.0, 1.0, 0.0, 0.0, None, None);

        // First call
        pid.compute_correction(80.0); // error = 20
        let accumulated1 = pid.accumulated_error();
        assert_eq!(accumulated1, 20.0);

        // Second call
        pid.compute_correction(80.0); // error = 20 again
        let accumulated2 = pid.accumulated_error();
        assert_eq!(accumulated2, 40.0); // Should accumulate

        // Third call
        pid.compute_correction(80.0);
        let accumulated3 = pid.accumulated_error();
        assert_eq!(accumulated3, 60.0); // Should keep accumulating
    }

    #[test]
    fn test_setpoint_change_response() {
        // Start with one setpoint, then create new controller with different setpoint
        // Verify that PID adapts correctly
        let mut pid = create_pid_controller(100.0, 1.0, 0.5, 0.2, 0.0, None, None);

        // Run a few iterations at setpoint 100
        for _ in 0..5 {
            pid.compute_correction(80.0);
        }

        let accumulated_before = pid.accumulated_error();
        assert!(accumulated_before > 0.0);

        // Change setpoint by creating new controller
        let mut pid_new = create_pid_controller(120.0, 1.0, 0.5, 0.2, 0.0, None, None);

        // At new setpoint 120, process_var 80 gives error = 40 (larger)
        let correction = pid_new.compute_correction(80.0);
        // P = 1.0 * 40 = 40
        // I = 0.5 * 40 = 20
        // D = 0.2 * (40 - 0) = 8
        // Total = 68
        assert_eq!(correction, 68.0);
    }

    #[test]
    fn test_proportional_term_scaling() {
        // Verify proportional term scales linearly with error magnitude
        let mut pid = create_pid_controller(100.0, 2.0, 0.0, 0.0, 0.0, None, None);

        let correction_small = pid.compute_correction(95.0); // error = 5
        let mut pid2 = create_pid_controller(100.0, 2.0, 0.0, 0.0, 0.0, None, None);
        let correction_large = pid2.compute_correction(90.0); // error = 10

        // Should scale: 10/5 = 2x
        assert!((correction_large / correction_small - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_integral_accumulation_over_time() {
        // Verify integral accumulates correctly over multiple calls
        let mut pid = create_pid_controller(100.0, 0.0, 1.0, 0.0, 0.0, None, None);

        let errors = [10.0, 20.0, 15.0, 5.0];
        let expected_accumulated = [10.0, 30.0, 45.0, 50.0];

        for (i, &error_val) in errors.iter().enumerate() {
            let process_var = 100.0 - error_val;
            pid.compute_correction(process_var);

            assert_eq!(
                pid.accumulated_error(),
                expected_accumulated[i],
                "Accumulated error mismatch at iteration {}",
                i
            );
        }
    }

    #[test]
    fn test_derivative_responds_to_rate_of_change() {
        // Derivative should respond to how fast the error is changing
        let mut pid = create_pid_controller(100.0, 0.0, 0.0, 1.0, 0.0, None, None);

        // Fast change: error goes from 0 to 50
        pid.compute_correction(100.0); // error = 0, previous = 0
        let correction_fast = pid.compute_correction(50.0); // error = 50, change = 50
        assert_eq!(correction_fast, 50.0); // D = 1.0 * 50

        // Slow change: error goes from 50 to 55
        let correction_slow = pid.compute_correction(45.0); // error = 55, change = 5
        assert_eq!(correction_slow, 5.0); // D = 1.0 * 5
    }

    #[test]
    fn test_output_limit_symmetry() {
        // Output limit should be symmetric: +limit and -limit
        let output_limit = 15.0;

        let mut pid_pos =
            create_pid_controller(100.0, 10.0, 0.0, 0.0, 0.0, None, Some(output_limit));
        let correction_pos = pid_pos.compute_correction(0.0); // Large positive error
        assert_eq!(correction_pos, output_limit);

        let mut pid_neg =
            create_pid_controller(100.0, 10.0, 0.0, 0.0, 0.0, None, Some(output_limit));
        let correction_neg = pid_neg.compute_correction(200.0); // Large negative error
        assert_eq!(correction_neg, -output_limit);
    }

    #[test]
    fn test_zero_ki_prevents_integral_accumulation() {
        // With ki=0, integral term should not accumulate
        let mut pid = create_pid_controller(100.0, 1.0, 0.0, 0.0, 0.0, None, None);

        for _ in 0..10 {
            pid.compute_correction(80.0); // Constant error
        }

        // Even though error bias would try to accumulate, ki=0 means no integral action
        // But accumulated_error still tracks the biased error internally
        // The contribution to output is ki * accumulated_error = 0 * accumulated = 0
        let correction = pid.compute_correction(80.0);
        // With ki=0, only P term: 1.0 * 20 = 20
        assert_eq!(correction, 20.0);
    }

    #[test]
    fn test_anti_windup_prevents_overshoot() {
        // Run scenario where anti-windup is critical
        let mut pid = create_pid_controller(100.0, 1.0, 0.5, 0.1, 0.0, None, Some(20.0));

        // Create large error to cause clamping
        for _ in 0..10 {
            pid.compute_correction(0.0); // error = 100, would cause huge integral
        }

        // Without anti-windup, accumulated_error would be 1000
        // With anti-windup, it should be much smaller
        let accumulated = pid.accumulated_error();
        assert!(
            accumulated < 200.0,
            "Anti-windup failed: accumulated_error {} too large",
            accumulated
        );
    }
}
