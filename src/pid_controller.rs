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
}
