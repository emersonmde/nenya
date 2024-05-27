#[derive(Debug)]
pub struct PIDController<T> {
    setpoint: T,
    kp: T,
    ki: T,
    kd: T,
    error_limit: T,
    output_limit: T,
    accumulated_error: T,
    previous_error: T,
    error_bias: T,
}

impl<T: num_traits::Signed + PartialOrd + Copy> PIDController<T> {
    pub fn new(
        setpoint: T,
        kp: T,
        ki: T,
        kd: T,
        error_limit: T,
        error_bias: T,
        output_limit: T,
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

    pub fn compute_correction(&mut self, signal: impl Into<T>) -> T {
        let error = self.setpoint - signal.into();
        let p = self.kp * error;

        // Bias error direction
        if error.is_positive() {
            self.accumulated_error = self.accumulated_error + error * self.error_bias.abs();
        } else {
            self.accumulated_error = self.accumulated_error + error / self.error_bias.abs();
        }

        // Clamp accumulated_error to prevent integral windup
        self.accumulated_error = num_traits::clamp(
            self.accumulated_error,
            -self.error_limit.abs(),
            self.error_limit.abs(),
        );

        let i = self.ki * self.accumulated_error;
        let d = self.kd * (error - self.previous_error);

        let correction = p + i + d;
        let clamped_correction = num_traits::clamp(
            correction,
            -self.output_limit.abs(),
            self.output_limit.abs(),
        );

        // Anti-windup feedback correction
        if correction != clamped_correction {
            let feedback = correction - clamped_correction;
            self.accumulated_error = self.accumulated_error - (feedback / self.ki);
        }

        self.previous_error = error;

        clamped_correction
    }

    pub fn accumulated_error(&self) -> T {
        self.accumulated_error
    }

    pub fn setpoint(&self) -> T {
        self.setpoint
    }
}

#[cfg(test)]
mod tests {
    use super::PIDController;

    #[test]
    fn test_pid_new() {
        let pid = PIDController::new(1.0, 1.0, 1.0, 1.0, 10.0, 1.0, 100.0);
        assert_eq!(pid.setpoint(), 1.0);
        assert_eq!(pid.accumulated_error(), 0.0);
    }

    #[test]
    fn test_pid_correction_positive_error() {
        let mut pid = PIDController::new(1.0, 1.0, 1.0, 1.0, 10.0, 1.0, 100.0);
        let correction = pid.compute_correction(0.0);
        assert!(
            correction > 0.0,
            "Correction should be positive when signal is below setpoint"
        );
    }

    #[test]
    fn test_pid_correction_negative_error() {
        let mut pid = PIDController::new(0.0, 1.0, 1.0, 1.0, 10.0, 1.0, 100.0);
        let correction = pid.compute_correction(1.0);
        assert!(
            correction < 0.0,
            "Correction should be negative when signal is above setpoint"
        );
    }

    #[test]
    fn test_pid_integral_windup() {
        let mut pid = PIDController::new(0.0, 0.0, 1.0, 0.0, 10.0, 1.0, 100.0);
        for _ in 0..20 {
            pid.compute_correction(1.0);
        }
        assert_eq!(
            pid.accumulated_error(),
            -10.0,
            "Accumulated error should be clamped to error_limit"
        );
    }

    #[test]
    fn test_pid_output_clamping() {
        let mut pid = PIDController::new(0.0, 100.0, 0.0, 0.0, 10.0, 1.0, 50.0);
        let correction = pid.compute_correction(1.0);
        assert_eq!(
            correction, -50.0,
            "Correction should be clamped to output_limit"
        );
    }

    #[test]
    fn test_pid_anti_windup_feedback() {
        let mut pid = PIDController::new(0.0, 1.0, 1.0, 0.0, 10.0, 1.0, 1.0);
        pid.compute_correction(10.0); // Large error
        assert!(
            pid.accumulated_error() < 10.0,
            "Accumulated error should be reduced by anti-windup feedback"
        );
    }

    #[test]
    fn test_pid_no_correction_needed() {
        let mut pid = PIDController::new(1.0, 1.0, 1.0, 1.0, 10.0, 1.0, 100.0);
        let correction = pid.compute_correction(1.0);
        assert_eq!(
            correction, 0.0,
            "Correction should be zero when signal equals setpoint"
        );
    }
}
