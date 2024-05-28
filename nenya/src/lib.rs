use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::pid_controller::PIDController;

pub mod pid_controller;

#[derive(Debug)]
pub struct RateLimiter {
    request_rate: f64,
    accepted_request_rate: f64,
    target_rate: f64,
    min_rate: f64,
    max_rate: f64,
    pid_controller: PIDController<f64>,
    last_updated: Instant,
    previous_output: f64,
    update_interval: Duration,
    request_timestamps: VecDeque<Instant>,
    accepted_request_timestamps: VecDeque<Instant>,
    external_request_rate: f64,
    external_accepted_request_rate: f64,
}

impl RateLimiter {
    pub fn new(
        target_rate: f64,
        min_rate: f64,
        max_rate: f64,
        pid_controller: PIDController<f64>,
        update_interval: Duration,
    ) -> RateLimiter {
        RateLimiter {
            request_rate: 0.0,
            accepted_request_rate: 0.0,
            target_rate,
            min_rate,
            max_rate,
            pid_controller,
            last_updated: Instant::now(),
            previous_output: 0.0,
            update_interval,
            request_timestamps: VecDeque::new(),
            accepted_request_timestamps: VecDeque::new(),
            external_request_rate: 0.0,
            external_accepted_request_rate: 0.0,
        }
    }

    pub fn should_throttle(&mut self) -> bool {
        let now = Instant::now();
        self.trim_request_window(now);
        self.calculate_request_rate(now);

        // Update PID controller and target rate periodically
        if now.duration_since(self.last_updated) > self.update_interval {
            self.last_updated = now;

            let output = self.pid_controller.compute_correction(self.request_rate);
            self.previous_output = output;

            self.target_rate =
                num_traits::clamp(self.target_rate + output, self.min_rate, self.max_rate);
        }

        // Make a throttling decision based on the target rate
        let should_handle_request = self.accepted_request_rate <= self.target_rate;
        if should_handle_request {
            self.accepted_request_timestamps.push_back(now);
        }
        self.request_timestamps.push_back(now);

        should_handle_request
    }

    fn calculate_request_rate(&mut self, now: Instant) {
        if let Some(&oldest) = self.accepted_request_timestamps.front() {
            let window_duration = now.duration_since(oldest).as_secs_f64();
            self.accepted_request_rate = if window_duration > 0.0 {
                self.accepted_request_timestamps.len() as f64 / window_duration
            } else {
                0.0
            };
        } else {
            self.accepted_request_rate = 0.0;
        }
        self.accepted_request_rate += self.external_accepted_request_rate;

        if let Some(&oldest) = self.request_timestamps.front() {
            let window_duration = now.duration_since(oldest).as_secs_f64();
            self.request_rate = if window_duration > 0.0 {
                self.request_timestamps.len() as f64 / window_duration
            } else {
                0.0
            };
        } else {
            self.request_rate = 0.0;
        }
        self.request_rate += self.external_request_rate;
    }

    fn trim_request_window(&mut self, now: Instant) {
        while let Some(timestamp) = self.accepted_request_timestamps.front() {
            if now.duration_since(*timestamp) > self.update_interval {
                self.accepted_request_timestamps.pop_front();
            } else {
                break;
            }
        }
        while let Some(timestamp) = self.request_timestamps.front() {
            if now.duration_since(*timestamp) > self.update_interval {
                self.request_timestamps.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn setpoint(&self) -> f64 {
        self.pid_controller.setpoint()
    }

    pub fn target_rate(&self) -> f64 {
        self.target_rate
    }

    pub fn request_rate(&self) -> f64 {
        self.request_rate
    }

    pub fn accepted_request_rate(&self) -> f64 {
        self.accepted_request_rate
    }

    pub fn external_request_rate(&self) -> f64 {
        self.external_request_rate
    }

    pub fn set_external_request_rate(&mut self, external_request_rate: impl Into<f64>) {
        self.external_request_rate = external_request_rate.into()
    }

    pub fn external_accepted_request_rate(&self) -> f64 {
        self.external_accepted_request_rate
    }

    pub fn set_external_accepted_request_rate(
        &mut self,
        external_accepted_request_rate: impl Into<f64>,
    ) {
        self.external_accepted_request_rate = external_accepted_request_rate.into()
    }
}
