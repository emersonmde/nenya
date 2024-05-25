use std::collections::VecDeque;
use std::time::{Duration, Instant};

// use pid::Pid;
use crate::pid_controller::PIDController;

pub mod pid_controller;

#[derive(Debug)]
pub struct RateLimiter {
    pub request_rate: f64,
    pub target_rate: f64,
    pub min_rate: f64,
    pub max_rate: f64,
    pub pid_controller: PIDController<f64>,
    last_updated: Instant,
    pub previous_output: f64,
    update_interval: Duration,
    accepted_request_timestamps: VecDeque<Instant>,
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
            request_rate: target_rate,
            target_rate,
            min_rate,
            max_rate,
            pid_controller,
            last_updated: Instant::now(),
            previous_output: 0.0,
            update_interval,
            accepted_request_timestamps: VecDeque::new(),
        }
    }

    pub fn handle_request(&mut self) -> bool {
        let now = Instant::now();

        while let Some(timestamp) = self.accepted_request_timestamps.front() {
            if timestamp.duration_since(now) > self.update_interval {
                self.accepted_request_timestamps.pop_front();
            } else {
                break;
            }
        }

        // Calculate current request rate over the moving window
        let window_duration = now
            .duration_since(*self.accepted_request_timestamps.front().unwrap_or(&now))
            .as_millis();
        self.request_rate = if window_duration > 0 {
            self.accepted_request_timestamps.len() as f64 / (window_duration as f64 / 1000.0)
        } else {
            0.0
        };

        // Update PID controller and target rate periodically
        if now.duration_since(self.last_updated) >= self.update_interval {
            self.last_updated = now;

            let output = self.pid_controller.compute_correction(self.request_rate);
            self.previous_output = output;

            self.target_rate =
                num_traits::clamp(self.target_rate + output, self.min_rate, self.max_rate);
        }

        // Make a throttling decision based on the target rate
        let should_handle_request = self.request_rate <= self.target_rate;
        if should_handle_request {
            self.accepted_request_timestamps.push_back(now);
        }

        should_handle_request
    }
}
