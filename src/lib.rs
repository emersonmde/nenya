//! # Distributed Rate Limiting System
//!
//! This crate provides a rate limiter with integrated PID controller to manage request rates
//! dynamically. It is designed for distributed systems where controlling the rate of requests
//! is crucial for stability and performance.
//!
//! ## Example
//!
//! The following example demonstrates how to use the `RateLimiter` with the builder pattern to create
//! a rate limiter instance and make throttling decisions based on the request rate.
//!
//! ```rust
//! use nenya::RateLimiterBuilder;
//! use nenya::pid_controller::PIDControllerBuilder;
//! use std::time::Duration;
//!
//! // Create a PID controller with specific parameters
//! let pid_controller = PIDControllerBuilder::new(10.0)
//!     .kp(1.0)
//!     .ki(0.1)
//!     .kd(0.01)
//!     .build();
//!
//! // Create a rate limiter using the builder pattern
//! let mut rate_limiter = RateLimiterBuilder::new(10.0)
//!     .min_rate(5.0)
//!     .max_rate(15.0)
//!     .pid_controller(pid_controller)
//!     .update_interval(Duration::from_secs(1))
//!     .build();
//!
//! // Simulate request processing and check if throttling is necessary
//! for _ in 0..20 {
//!     if rate_limiter.should_throttle() {
//!         println!("Request throttled");
//!     } else {
//!         println!("Request accepted");
//!     }
//! }
//! ```

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct _README;

use num_traits::{Float, FromPrimitive, Signed};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::pid_controller::PIDController;

// ===== Public Library API =====
pub mod pid_controller;

// ===== Server modules (binary only, not part of public API) =====
#[cfg(feature = "server")]
pub(crate) mod api;

#[cfg(feature = "server")]
pub(crate) mod config;

#[cfg(feature = "server")]
pub(crate) mod gossip;

#[cfg(feature = "server")]
pub(crate) mod discovery;

/// Sliding window rate limiter with an integrated PID controller for dynamic target rate adjustment.
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
    local_accepted_request_rate: T, // Local rate only (for throttling decisions)
    external_request_rate: T,
    external_accepted_request_rate: T,
}

impl<T: Float + Signed + FromPrimitive + Copy> RateLimiter<T> {
    /// Creates a new `RateLimiter` instance.
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
            local_accepted_request_rate: T::zero(),
            external_request_rate: T::zero(),
            external_accepted_request_rate: T::zero(),
        }
    }

    /// Determines if the current request should be throttled based on the rate limiter's state.
    ///
    /// This method uses the current system time (`Instant::now()`). For testing or scenarios
    /// where you need to control time, use [`should_throttle_at`](Self::should_throttle_at).
    ///
    /// Returns `true` if the request should be throttled, `false` otherwise.
    pub fn should_throttle(&mut self) -> bool {
        self.should_throttle_at(Instant::now())
    }

    /// Determines if a request at the given timestamp should be throttled.
    ///
    /// This is the core rate limiting logic with injectable time. The timestamp parameter
    /// allows for:
    /// - Deterministic testing without actual time passing
    /// - Replaying historical request patterns
    /// - Simulating distributed request timestamps
    ///
    /// # Arguments
    /// * `now` - The timestamp of the current request
    ///
    /// # Returns
    /// `true` if the request should be throttled, `false` otherwise.
    ///
    /// # Example
    /// ```rust
    /// use nenya::RateLimiterBuilder;
    /// use nenya::pid_controller::PIDControllerBuilder;
    /// use std::time::{Duration, Instant};
    ///
    /// let pid = PIDControllerBuilder::new(100.0).kp(0.8).build();
    /// let mut limiter = RateLimiterBuilder::new(100.0)
    ///     .min_rate(50.0)
    ///     .max_rate(150.0)
    ///     .pid_controller(pid)
    ///     .update_interval(Duration::from_millis(100))
    ///     .build();
    ///
    /// let base_time = Instant::now();
    /// // Simulate requests with controlled timestamps
    /// for i in 0..10 {
    ///     let request_time = base_time + Duration::from_millis(i * 10);
    ///     let throttled = limiter.should_throttle_at(request_time);
    ///     println!("Request {}: throttled={}", i, throttled);
    /// }
    /// ```
    pub fn should_throttle_at(&mut self, now: Instant) -> bool {
        self.trim_request_window(now);
        self.calculate_request_rate(now);

        // Update PID controller and target rate periodically
        if now.duration_since(self.last_updated) > self.update_interval {
            self.last_updated = now;

            // Use accepted_request_rate as the process variable for PID control
            // We want to control the accepted rate to match the setpoint,
            // not the incoming request rate (which we cannot control)
            let output = self
                .pid_controller
                .compute_correction(self.accepted_request_rate);
            self.previous_output = output;

            self.target_rate =
                num_traits::clamp(self.target_rate + output, self.min_rate, self.max_rate);
        }

        // Make a throttling decision based on the target rate
        // Use LOCAL accepted rate for throttling (not combined with external)
        let should_handle_request = self.local_accepted_request_rate <= self.target_rate;
        if should_handle_request {
            self.accepted_request_timestamps.push_back(now);
        }
        self.request_timestamps.push_back(now);

        !should_handle_request
    }

    /// Updates internal state (trims windows, updates PID) at the given timestamp
    /// without recording an actual request.
    ///
    /// This is useful for test scenarios where you want to keep the rate limiter's
    /// state current during periods of zero load, without polluting the request windows
    /// with fake timestamps.
    ///
    /// In production code, you typically only call `should_throttle()` or `should_throttle_at()`
    /// when actual requests arrive.
    pub fn update_state_at(&mut self, now: Instant) {
        self.trim_request_window(now);
        self.calculate_request_rate(now);

        // Update PID controller and target rate periodically
        if now.duration_since(self.last_updated) > self.update_interval {
            self.last_updated = now;

            let output = self
                .pid_controller
                .compute_correction(self.accepted_request_rate);
            self.previous_output = output;

            self.target_rate =
                num_traits::clamp(self.target_rate + output, self.min_rate, self.max_rate);
        }
    }

    /// Calculates the current request rate based on the timestamps of recent requests.
    fn calculate_request_rate(&mut self, now: Instant) {
        let min_duration = 0.1; // Minimum duration threshold in seconds

        // Calculate local accepted rate (from our own timestamps only)
        self.local_accepted_request_rate =
            if let Some(&oldest) = self.accepted_request_timestamps.front() {
                let window_duration = now.duration_since(oldest).as_secs_f32();
                let effective_duration = if window_duration < min_duration {
                    min_duration
                } else {
                    window_duration
                };

                if T::from_f32(effective_duration).unwrap() > T::zero() {
                    T::from_usize(self.accepted_request_timestamps.len()).unwrap()
                        / T::from_f32(effective_duration).unwrap()
                } else {
                    T::zero()
                }
            } else {
                T::zero()
            };

        // Combine local + external for PID control
        self.accepted_request_rate =
            self.local_accepted_request_rate + self.external_accepted_request_rate;

        if let Some(&oldest) = self.request_timestamps.front() {
            let window_duration = now.duration_since(oldest).as_secs_f32();
            let effective_duration = if window_duration < min_duration {
                min_duration
            } else {
                window_duration
            };

            self.request_rate = if T::from_f32(effective_duration).unwrap() > T::zero() {
                T::from_usize(self.request_timestamps.len()).unwrap()
                    / T::from_f32(effective_duration).unwrap()
            } else {
                T::zero()
            };
        } else {
            self.request_rate = T::zero();
        }
        self.request_rate = self.request_rate + self.external_request_rate;
    }

    /// Trims old request timestamps that are outside the update interval.
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

    /// Returns the current setpoint of the PID controller.
    pub fn setpoint(&self) -> T {
        self.pid_controller.setpoint()
    }

    /// Returns the current target rate of the rate limiter.
    pub fn target_rate(&self) -> T {
        self.target_rate
    }

    /// Returns the current request rate.
    pub fn request_rate(&self) -> T {
        self.request_rate
    }

    /// Returns the current accepted request rate (combined local + external).
    pub fn accepted_request_rate(&self) -> T {
        self.accepted_request_rate
    }

    /// Returns the local accepted request rate (excluding external rates).
    pub fn local_accepted_request_rate(&self) -> T {
        self.local_accepted_request_rate
    }

    /// Returns the current external request rate.
    pub fn external_request_rate(&self) -> T {
        self.external_request_rate
    }

    /// Sets the external request rate.
    pub fn set_external_request_rate(&mut self, external_request_rate: impl Into<T>) {
        self.external_request_rate = external_request_rate.into()
    }

    /// Returns the current external accepted request rate.
    pub fn external_accepted_request_rate(&self) -> T {
        self.external_accepted_request_rate
    }

    /// Sets the external accepted request rate.
    pub fn set_external_accepted_request_rate(
        &mut self,
        external_accepted_request_rate: impl Into<T>,
    ) {
        self.external_accepted_request_rate = external_accepted_request_rate.into()
    }
}

/// Builder for creating a `RateLimiter` instance.
pub struct RateLimiterBuilder<T> {
    target_rate: T,
    min_rate: T,
    max_rate: T,
    pid_controller: Option<PIDController<T>>,
    update_interval: Duration,
    external_request_rate: T,
    external_accepted_request_rate: T,
    initial_timestamp: Option<Instant>,
}

impl<T: Float + Signed + FromPrimitive + Copy> RateLimiterBuilder<T> {
    /// Creates a new `RateLimiterBuilder` with default values.
    pub fn new(target_rate: T) -> Self {
        RateLimiterBuilder {
            target_rate,
            min_rate: target_rate,
            max_rate: target_rate,
            pid_controller: None,
            update_interval: Duration::from_secs(1),
            external_request_rate: T::zero(),
            external_accepted_request_rate: T::zero(),
            initial_timestamp: None,
        }
    }

    /// Sets the minimum allowable rate of requests.
    pub fn min_rate(mut self, min_rate: T) -> Self {
        self.min_rate = min_rate;
        self
    }

    /// Sets the maximum allowable rate of requests.
    pub fn max_rate(mut self, max_rate: T) -> Self {
        self.max_rate = max_rate;
        self
    }

    /// Sets the PID controller for the rate limiter.
    pub fn pid_controller(mut self, pid_controller: PIDController<T>) -> Self {
        self.pid_controller = Some(pid_controller);
        self
    }

    /// Sets the update interval for the PID controller.
    pub fn update_interval(mut self, update_interval: Duration) -> Self {
        self.update_interval = update_interval;
        self
    }

    /// Sets the external request rate.
    pub fn external_request_rate(mut self, external_request_rate: T) -> Self {
        self.external_request_rate = external_request_rate;
        self
    }

    /// Sets the external accepted request rate.
    pub fn external_accepted_request_rate(mut self, external_accepted_request_rate: T) -> Self {
        self.external_accepted_request_rate = external_accepted_request_rate;
        self
    }

    /// Sets the initial timestamp for testing scenarios with controlled time.
    ///
    /// When using `should_throttle_at()` with controlled timestamps, you may want to
    /// set an initial timestamp that's earlier than your first request to ensure the
    /// PID controller can update properly.
    ///
    /// # Example
    /// ```rust
    /// use nenya::RateLimiterBuilder;
    /// use nenya::pid_controller::PIDControllerBuilder;
    /// use std::time::{Duration, Instant};
    ///
    /// let base_time = Instant::now();
    /// let pid = PIDControllerBuilder::new(100.0).kp(0.8).build();
    ///
    /// // Initialize limiter with a timestamp 1 second in the past
    /// let mut limiter = RateLimiterBuilder::new(100.0)
    ///     .pid_controller(pid)
    ///     .initial_timestamp(base_time - Duration::from_secs(1))
    ///     .build();
    ///
    /// // Now requests at base_time will trigger PID updates
    /// let throttled = limiter.should_throttle_at(base_time);
    /// ```
    pub fn initial_timestamp(mut self, timestamp: Instant) -> Self {
        self.initial_timestamp = Some(timestamp);
        self
    }

    /// Builds and returns the `RateLimiter` instance.
    pub fn build(self) -> RateLimiter<T> {
        RateLimiter {
            request_rate: T::zero(),
            accepted_request_rate: T::zero(),
            target_rate: self.target_rate,
            min_rate: self.min_rate,
            max_rate: self.max_rate,
            pid_controller: self
                .pid_controller
                .unwrap_or_else(|| PIDController::new_static_controller(self.target_rate)),
            last_updated: self.initial_timestamp.unwrap_or_else(Instant::now),
            previous_output: T::zero(),
            update_interval: self.update_interval,
            request_timestamps: VecDeque::new(),
            accepted_request_timestamps: VecDeque::new(),
            local_accepted_request_rate: T::zero(),
            external_request_rate: self.external_request_rate,
            external_accepted_request_rate: self.external_accepted_request_rate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pid_controller::PIDControllerBuilder;
    use num_traits::FromPrimitive;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// Utility function to create a RateLimiter with defaults
    fn create_rate_limiter<T: Float + Signed + FromPrimitive + Copy>(
        target_rate: T,
        min_rate: T,
        max_rate: T,
        pid_controller: PIDController<T>,
        update_interval: Duration,
    ) -> RateLimiter<T> {
        RateLimiter::new(
            target_rate,
            min_rate,
            max_rate,
            pid_controller,
            update_interval,
        )
    }

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
    fn test_rate_limiter_initialization() {
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        assert_eq!(rate_limiter.target_rate(), 10.0);
        assert_eq!(rate_limiter.min_rate, 5.0);
        assert_eq!(rate_limiter.max_rate, 15.0);
        assert_eq!(rate_limiter.request_rate(), 0.0);
        assert_eq!(rate_limiter.accepted_request_rate(), 0.0);
        assert!(rate_limiter.last_updated.elapsed().as_secs() <= 1);
        assert_eq!(rate_limiter.previous_output, 0.0);
        assert_eq!(rate_limiter.request_timestamps.len(), 0);
        assert_eq!(rate_limiter.accepted_request_timestamps.len(), 0);
    }

    #[test]
    fn test_should_throttle_under_load() {
        let pid = PIDController::new_static_controller(10.0);
        let mut rate_limiter = create_rate_limiter(10.0, 10.0, 10.0, pid, Duration::from_secs(1));

        for _ in 0..10 {
            let should_throttle = rate_limiter.should_throttle();
            assert!(!should_throttle);
            sleep(Duration::from_millis(100));
        }

        rate_limiter.should_throttle();
        rate_limiter.should_throttle();
        rate_limiter.should_throttle();

        for _ in 0..10 {
            let should_throttle = rate_limiter.should_throttle();
            assert!(should_throttle);
        }

        sleep(Duration::from_secs(2));

        for _ in 0..5 {
            let should_throttle = rate_limiter.should_throttle();
            assert!(!should_throttle);
            sleep(Duration::from_millis(100));
        }

        assert!(rate_limiter.request_rate() > 0.0);
        assert!(rate_limiter.accepted_request_rate() > 0.0);
        assert!(!rate_limiter.accepted_request_timestamps.is_empty());
        assert!(!rate_limiter.request_timestamps.is_empty());
    }

    #[test]
    fn test_should_throttle_under_load_with_external_tps() {
        let pid = PIDController::new_static_controller(10.0);
        let mut rate_limiter = create_rate_limiter(12.0, 12.0, 12.0, pid, Duration::from_secs(1));
        rate_limiter.set_external_request_rate(2.0);
        rate_limiter.set_external_accepted_request_rate(2.0);

        for _ in 0..10 {
            let should_throttle = rate_limiter.should_throttle();
            assert!(!should_throttle);
            sleep(Duration::from_millis(100));
        }

        rate_limiter.should_throttle();
        rate_limiter.should_throttle();
        rate_limiter.should_throttle();

        for _ in 0..10 {
            let should_throttle = rate_limiter.should_throttle();
            assert!(should_throttle);
        }

        sleep(Duration::from_secs(2));

        for _ in 0..5 {
            let should_throttle = rate_limiter.should_throttle();
            assert!(!should_throttle);
            sleep(Duration::from_millis(100));
        }

        assert!(rate_limiter.request_rate() > 0.0);
        assert!(rate_limiter.accepted_request_rate() > 0.0);
        assert!(!rate_limiter.accepted_request_timestamps.is_empty());
        assert!(!rate_limiter.request_timestamps.is_empty());
    }

    #[test]
    fn test_should_throttle_with_pid_adjustment() {
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        for _ in 0..20 {
            rate_limiter.should_throttle();
        }

        sleep(Duration::from_secs(2));

        let old_target_rate = rate_limiter.target_rate();
        rate_limiter.should_throttle();
        let new_target_rate = rate_limiter.target_rate();

        assert_ne!(new_target_rate, old_target_rate);
        assert!((5.0..=15.0).contains(&new_target_rate))
    }

    #[test]
    fn test_trim_request_window() {
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        let now = Instant::now();
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_secs(2));
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_secs(1));

        rate_limiter.trim_request_window(now);

        assert_eq!(rate_limiter.request_timestamps.len(), 1);
    }

    #[test]
    fn test_calculate_request_rate() {
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        let now = Instant::now();
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_secs(2));
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_secs(1));

        rate_limiter.calculate_request_rate(now);

        assert!(rate_limiter.request_rate() > 0.0);
    }

    #[test]
    fn test_external_rates() {
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        rate_limiter.set_external_request_rate(2.0);
        rate_limiter.set_external_accepted_request_rate(2.0);

        assert_eq!(rate_limiter.external_request_rate(), 2.0);
        assert_eq!(rate_limiter.external_accepted_request_rate(), 2.0);
    }

    #[test]
    fn test_request_rate_with_external_rate() {
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        rate_limiter.set_external_request_rate(2.0);

        let now = Instant::now();
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_secs(2));
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_secs(1));

        rate_limiter.calculate_request_rate(now);

        assert_eq!(rate_limiter.request_rate(), 2.0 + (2.0 / 2.0));
    }

    #[test]
    fn test_accepted_request_rate_with_external_rate() {
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        rate_limiter.set_external_accepted_request_rate(2.0);

        let now = Instant::now();
        rate_limiter
            .accepted_request_timestamps
            .push_back(now - Duration::from_secs(2));
        rate_limiter
            .accepted_request_timestamps
            .push_back(now - Duration::from_secs(1));

        rate_limiter.calculate_request_rate(now);

        assert_eq!(rate_limiter.accepted_request_rate(), 2.0 + (2.0 / 2.0));
    }

    // ===== Enhanced Unit Tests for Rate Limiter Components =====

    #[test]
    fn test_rate_calculation_minimum_duration_threshold() {
        // Verify that minimum duration threshold (0.1s) prevents division by tiny numbers
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(100.0, 50.0, 150.0, pid, Duration::from_secs(1));

        let now = Instant::now();

        // Add timestamps very close together (< 0.1s)
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_millis(50));
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_millis(40));
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_millis(30));

        rate_limiter.calculate_request_rate(now);

        // With 3 requests and actual duration 0.05s, without min threshold:
        // rate would be 3 / 0.05 = 60 TPS
        // With min threshold of 0.1s:
        // rate should be 3 / 0.1 = 30 TPS
        let calculated_rate = rate_limiter.request_rate();

        assert!(
            (calculated_rate - 30.0).abs() < 1.0,
            "Rate {} should use 0.1s minimum duration",
            calculated_rate
        );
    }

    #[test]
    fn test_sliding_window_trim_correctness() {
        // Verify that old timestamps outside the window are removed
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(100.0, 50.0, 150.0, pid, Duration::from_secs(2));

        let now = Instant::now();

        // Add timestamps spanning > window duration
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_secs(5)); // Too old
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_secs(3)); // Too old
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_secs(1)); // Recent
        rate_limiter.request_timestamps.push_back(now); // Recent

        rate_limiter.trim_request_window(now);

        // Only timestamps within last 2 seconds should remain
        assert_eq!(
            rate_limiter.request_timestamps.len(),
            2,
            "Should trim old timestamps"
        );

        // Verify the remaining ones are the recent ones
        assert!(
            rate_limiter.request_timestamps[0].elapsed().as_secs() <= 2,
            "First remaining timestamp should be recent"
        );
        assert!(
            rate_limiter.request_timestamps[1].elapsed().as_secs() <= 1,
            "Second remaining timestamp should be recent"
        );
    }

    #[test]
    fn test_external_rate_aggregation() {
        // Verify that external rates are added to local rates for PID input
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(100.0, 50.0, 150.0, pid, Duration::from_secs(1));

        let now = Instant::now();

        // Setup local rate: 2 requests over 1 second = 2 TPS
        rate_limiter
            .request_timestamps
            .push_back(now - Duration::from_secs(1));
        rate_limiter.request_timestamps.push_back(now);

        rate_limiter
            .accepted_request_timestamps
            .push_back(now - Duration::from_secs(1));
        rate_limiter.accepted_request_timestamps.push_back(now);

        // Set external rates
        rate_limiter.set_external_request_rate(30.0);
        rate_limiter.set_external_accepted_request_rate(30.0);

        rate_limiter.calculate_request_rate(now);

        // Total request rate should be local + external
        // Local: 2 requests / 1.0s = 2 TPS
        // External: 30 TPS
        // Total: 32 TPS
        let total_rate = rate_limiter.request_rate();
        assert!(
            (total_rate - 32.0).abs() < 1.0,
            "Total rate {} should be local(2) + external(30)",
            total_rate
        );

        let total_accepted = rate_limiter.accepted_request_rate();
        assert!(
            (total_accepted - 32.0).abs() < 1.0,
            "Total accepted {} should be local(2) + external(30)",
            total_accepted
        );
    }

    #[test]
    fn test_pid_update_interval_enforcement() {
        // Verify that PID only updates every update_interval
        let pid = create_pid_controller(1.0, 1.0, 0.1, 0.01, 0.0, None, None);
        let mut rate_limiter =
            create_rate_limiter(100.0, 50.0, 150.0, pid, Duration::from_millis(500));

        // Record initial target rate
        let initial_target = rate_limiter.target_rate();

        // Make many rapid calls (much faster than update_interval)
        for _ in 0..20 {
            rate_limiter.should_throttle();
        }

        // Target rate should not have changed yet (no time passed)
        let target_after_rapid = rate_limiter.target_rate();
        assert_eq!(
            target_after_rapid, initial_target,
            "Target rate should not change without time passing"
        );

        // Wait for update interval to pass
        sleep(Duration::from_millis(600));

        // Generate some load to create error
        for _ in 0..10 {
            rate_limiter.should_throttle();
        }

        // Now PID should have updated
        let target_after_wait = rate_limiter.target_rate();
        // Target should have changed (PID adjusted)
        // Can't predict exact value, but should be different
        assert!(
            (target_after_wait - initial_target).abs() > 0.01,
            "Target rate should change after update_interval passes"
        );
    }

    #[test]
    fn test_throttle_decision_threshold() {
        // Verify throttling decision is based on: accepted_rate <= target_rate
        let pid = PIDController::new_static_controller(10.0);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        // At start, accepted_rate = 0, should accept requests
        // Space out requests to keep rate under target (10 TPS = 100ms per request)
        for _ in 0..5 {
            let throttled = rate_limiter.should_throttle();
            assert!(!throttled, "Should accept when accepted_rate < target_rate");
            sleep(Duration::from_millis(150)); // Slower than 10 TPS
        }

        // After many requests in quick succession, should start throttling
        for _ in 0..20 {
            rate_limiter.should_throttle();
        }

        // Should throttle when rate exceeds target
        let should_throttle = rate_limiter.should_throttle();
        assert!(
            should_throttle,
            "Should throttle when accepted_rate > target_rate"
        );

        // Wait for window to reset
        sleep(Duration::from_secs(2));

        // Should accept again
        for _ in 0..3 {
            let throttled = rate_limiter.should_throttle();
            assert!(!throttled, "Should accept again after window resets");
            sleep(Duration::from_millis(200));
        }
    }
}
