#[derive(Debug)]
pub struct PIDController<T> {
    pub setpoint: T,
    kp: T,
    ki: T,
    kd: T,
    error_limit: T,
    output_limit: T,
    pub accumulated_error: T,
    previous_error: T,
}

impl<T: num_traits::Signed + PartialOrd + Copy> PIDController<T> {
    pub fn new(setpoint: T, kp: T, ki: T, kd: T, error_limit: T, output_limit: T) -> Self {
        PIDController {
            setpoint,
            kp,
            ki,
            kd,
            error_limit,
            output_limit,
            accumulated_error: T::zero(),
            previous_error: T::zero(),
        }
    }

    pub fn compute_correction(&mut self, signal: impl Into<T>) -> T {
        let error = self.setpoint - signal.into();
        let p = self.kp * error;

        // Clamp accumulated_error to prevent integral windup
        self.accumulated_error = num_traits::clamp(
            self.accumulated_error + error,
            -self.error_limit.abs(),
            self.error_limit.abs(),
        );
        let i = self.ki * self.accumulated_error;
        let d = self.kd * (error - self.previous_error);

        self.previous_error = error;

        num_traits::clamp(p + i + d, -self.output_limit.abs(), self.output_limit.abs())
    }
}
