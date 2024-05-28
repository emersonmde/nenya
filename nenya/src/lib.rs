use num_traits::{FromPrimitive, Signed, Zero};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::pid_controller::PIDController;

pub mod pid_controller;

#[derive(Debug)]
pub struct RateLimiter<T> {
    request_rate: T,
    accepted_request_rate: T,
    target_rate: T,
    min_rate: T,
    max_rate: T,
    pid_controller: PIDController<T>,
    last_updated: Instant,
    previous_output: T,
    update_interval: Duration,
    request_timestamps: VecDeque<Instant>,
    accepted_request_timestamps: VecDeque<Instant>,
    external_request_rate: T,
    external_accepted_request_rate: T,
}

impl<T: Signed + PartialOrd + Zero + Copy + FromPrimitive> RateLimiter<T> {
    pub fn new(
        target_rate: T,
        min_rate: T,
        max_rate: T,
        pid_controller: PIDController<T>,
        update_interval: Duration,
    ) -> RateLimiter<T> {
        RateLimiter {
            request_rate: T::zero(),
            accepted_request_rate: T::zero(),
            target_rate,
            min_rate,
            max_rate,
            pid_controller,
            last_updated: Instant::now(),
            previous_output: T::zero(),
            update_interval,
            request_timestamps: VecDeque::new(),
            accepted_request_timestamps: VecDeque::new(),
            external_request_rate: T::zero(),
            external_accepted_request_rate: T::zero(),
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
            let window_duration = now.duration_since(oldest).as_secs_f32();
            self.accepted_request_rate = if T::from_f32(window_duration).unwrap() > T::zero() {
                T::from_usize(self.accepted_request_timestamps.len()).unwrap()
                    / T::from_f32(window_duration).unwrap()
            } else {
                T::zero()
            };
        } else {
            self.accepted_request_rate = T::zero();
        }
        self.accepted_request_rate =
            self.accepted_request_rate + self.external_accepted_request_rate;

        if let Some(&oldest) = self.request_timestamps.front() {
            let window_duration = now.duration_since(oldest).as_secs_f32();
            self.request_rate = if T::from_f32(window_duration).unwrap() > T::zero() {
                T::from_usize(self.request_timestamps.len()).unwrap()
                    / T::from_f32(window_duration).unwrap()
            } else {
                T::zero()
            };
        } else {
            self.request_rate = T::zero();
        }
        self.request_rate = self.request_rate + self.external_request_rate;
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

    pub fn setpoint(&self) -> T {
        self.pid_controller.setpoint()
    }

    pub fn target_rate(&self) -> T {
        self.target_rate
    }

    pub fn request_rate(&self) -> T {
        self.request_rate
    }

    pub fn accepted_request_rate(&self) -> T {
        self.accepted_request_rate
    }

    pub fn external_request_rate(&self) -> T {
        self.external_request_rate
    }

    pub fn set_external_request_rate(&mut self, external_request_rate: impl Into<T>) {
        self.external_request_rate = external_request_rate.into()
    }

    pub fn external_accepted_request_rate(&self) -> T {
        self.external_accepted_request_rate
    }

    pub fn set_external_accepted_request_rate(
        &mut self,
        external_accepted_request_rate: impl Into<T>,
    ) {
        self.external_accepted_request_rate = external_accepted_request_rate.into()
    }
}
