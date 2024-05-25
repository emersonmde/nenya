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
