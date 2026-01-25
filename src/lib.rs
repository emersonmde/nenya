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

/// Token bucket rate limiter with sliding window measurement and PID controller for adaptive coordination.
///
/// This hybrid architecture uses:
/// - Token bucket for precise, collision-immune throttling decisions
/// - Sliding window for accurate rate measurement (PID feedback)
/// - PID controller for adaptive distributed coordination
#[derive(Debug)]
pub struct RateLimiter<T> {
    // Token Bucket state (local throttling)
    tokens: T,
    last_refill: Instant,
    refill_rate: T,
    bucket_capacity: T,

    // Sliding Window state (rate measurement)
    accepted_request_timestamps: VecDeque<Instant>,
    local_accepted_request_rate: T,

    // PID Control state (adaptive coordination)
    pid_controller: PIDController<T>,
    target_rate: T,
    min_rate: T,
    max_rate: T,
    update_interval: Duration,
    last_updated: Instant,

    // External rates (distributed coordination)
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
        bucket_capacity: T,
    ) -> RateLimiter<T> {
        let now = Instant::now();
        RateLimiter {
            // Token bucket initialized with full capacity
            tokens: bucket_capacity,
            last_refill: now,
            refill_rate: target_rate,
            bucket_capacity,

            // Sliding window starts empty
            accepted_request_timestamps: VecDeque::new(),
            local_accepted_request_rate: T::zero(),

            // PID control
            pid_controller,
            target_rate,
            min_rate,
            max_rate,
            update_interval,
            last_updated: now,

            // External rates
            external_accepted_request_rate: T::zero(),
        }
    }

    /// Refills the token bucket based on elapsed time since last refill.
    ///
    /// Tokens are added based on: elapsed_time * refill_rate
    /// The token count is capped at bucket_capacity to prevent unbounded burst.
    fn refill_tokens(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();

        // Calculate new tokens based on refill rate
        let new_tokens = T::from_f64(elapsed).unwrap() * self.refill_rate;
        self.tokens = (self.tokens + new_tokens).min(self.bucket_capacity);

        self.last_refill = now;
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
    /// This is the core rate limiting logic with injectable time. Uses a token bucket
    /// for precise throttling decisions, immune to timestamp collisions.
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
        // 1. Refill tokens based on elapsed time
        self.refill_tokens(now);

        // 2. Periodically update PID control (regardless of accept/throttle decision)
        if now.duration_since(self.last_updated) >= self.update_interval {
            self.update_pid_control(now);
        }

        // 3. Make throttle decision (token bucket)
        if self.tokens >= T::one() {
            self.tokens = self.tokens - T::one();

            // Record acceptance for rate measurement
            self.accepted_request_timestamps.push_back(now);
            self.trim_accepted_timestamps(now);

            false // Accept request
        } else {
            true // Throttle request
        }
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
        self.refill_tokens(now);
        self.trim_accepted_timestamps(now);
        self.calculate_local_accepted_rate(now);

        // Update PID controller periodically
        if now.duration_since(self.last_updated) >= self.update_interval {
            self.update_pid_control(now);
        }
    }

    /// Updates PID control by measuring rate and adjusting refill_rate.
    ///
    /// This method:
    /// 1. Measures local accepted rate from sliding window
    /// 2. Aggregates with external rates
    /// 3. Computes PID correction
    /// 4. Adjusts refill_rate (clamped to min/max bounds)
    fn update_pid_control(&mut self, now: Instant) {
        // Measure local accepted rate (sliding window)
        self.calculate_local_accepted_rate(now);

        // Aggregate with external rates
        let total_accepted_rate =
            self.local_accepted_request_rate + self.external_accepted_request_rate;

        // PID computes correction
        let correction = self.pid_controller.compute_correction(total_accepted_rate);

        // Adjust refill_rate (clamped to bounds)
        self.refill_rate =
            num_traits::clamp(self.target_rate + correction, self.min_rate, self.max_rate);

        self.last_updated = now;
    }

    /// Calculates the local accepted request rate based on the sliding window.
    ///
    /// Uses a smaller minimum duration threshold (1ms) compared to the old implementation,
    /// because token bucket throttling reduces timestamp collisions in the accepted stream.
    fn calculate_local_accepted_rate(&mut self, now: Instant) {
        if let Some(&oldest) = self.accepted_request_timestamps.front() {
            let duration = now.duration_since(oldest).as_secs_f64();

            // Smaller minimum duration acceptable (fewer collisions due to token bucket)
            let effective_duration = duration.max(0.001); // 1ms minimum

            self.local_accepted_request_rate =
                T::from_usize(self.accepted_request_timestamps.len()).unwrap()
                    / T::from_f64(effective_duration).unwrap();
        } else {
            self.local_accepted_request_rate = T::zero();
        }
    }

    /// Trims old accepted request timestamps that are outside the update interval.
    fn trim_accepted_timestamps(&mut self, now: Instant) {
        while let Some(timestamp) = self.accepted_request_timestamps.front() {
            if now.duration_since(*timestamp) > self.update_interval {
                self.accepted_request_timestamps.pop_front();
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

    /// Returns the current refill rate (adjusted by PID).
    pub fn refill_rate(&self) -> T {
        self.refill_rate
    }

    /// Returns the current token count.
    pub fn tokens(&self) -> T {
        self.tokens
    }

    /// Returns the current accepted request rate (combined local + external).
    pub fn accepted_request_rate(&self) -> T {
        self.local_accepted_request_rate + self.external_accepted_request_rate
    }

    /// Returns the local accepted request rate (excluding external rates).
    pub fn local_accepted_request_rate(&self) -> T {
        self.local_accepted_request_rate
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
    bucket_capacity: Option<T>,
    pid_controller: Option<PIDController<T>>,
    update_interval: Duration,
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
            bucket_capacity: None,
            pid_controller: None,
            update_interval: Duration::from_secs(1),
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

    /// Sets the token bucket capacity.
    ///
    /// The bucket capacity determines the maximum burst size. Defaults to target_rate * 1.0
    /// (allowing a 1 second burst at target rate).
    ///
    /// # Example
    /// ```rust
    /// use nenya::RateLimiterBuilder;
    ///
    /// // Allow 2-second burst at 100 RPS target
    /// let limiter = RateLimiterBuilder::new(100.0)
    ///     .bucket_capacity(200.0)
    ///     .build();
    /// ```
    pub fn bucket_capacity(mut self, capacity: T) -> Self {
        self.bucket_capacity = Some(capacity);
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
        let now = self.initial_timestamp.unwrap_or_else(Instant::now);
        let bucket_capacity = self.bucket_capacity.unwrap_or(self.target_rate);

        RateLimiter {
            // Token bucket initialized with full capacity
            tokens: bucket_capacity,
            last_refill: now,
            refill_rate: self.target_rate,
            bucket_capacity,

            // Sliding window starts empty
            accepted_request_timestamps: VecDeque::new(),
            local_accepted_request_rate: T::zero(),

            // PID control
            pid_controller: self
                .pid_controller
                .unwrap_or_else(|| PIDController::new_static_controller(self.target_rate)),
            target_rate: self.target_rate,
            min_rate: self.min_rate,
            max_rate: self.max_rate,
            update_interval: self.update_interval,
            last_updated: now,

            // External rates
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
            target_rate, // Default bucket_capacity = target_rate (1 second burst)
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

        // Token bucket state
        assert_eq!(rate_limiter.tokens(), 10.0); // Initialized to bucket_capacity
        assert_eq!(rate_limiter.refill_rate(), 10.0); // Initialized to target_rate
        assert_eq!(rate_limiter.bucket_capacity, 10.0);
        assert!(rate_limiter.last_refill.elapsed().as_secs() <= 1);

        // Rate state
        assert_eq!(rate_limiter.target_rate(), 10.0);
        assert_eq!(rate_limiter.min_rate, 5.0);
        assert_eq!(rate_limiter.max_rate, 15.0);
        assert_eq!(rate_limiter.accepted_request_rate(), 0.0);
        assert_eq!(rate_limiter.local_accepted_request_rate(), 0.0);
        assert!(rate_limiter.last_updated.elapsed().as_secs() <= 1);

        // Timestamps
        assert_eq!(rate_limiter.accepted_request_timestamps.len(), 0);
    }

    #[test]
    fn test_should_throttle_under_load() {
        let pid = PIDController::new_static_controller(10.0);
        let mut rate_limiter = create_rate_limiter(10.0, 10.0, 10.0, pid, Duration::from_secs(1));

        // Phase 1: Exhaust tokens with burst
        // Start with 10 tokens, consume them all
        for i in 0..10 {
            let should_throttle = rate_limiter.should_throttle();
            assert!(
                !should_throttle,
                "Should accept burst request {} (have tokens)",
                i
            );
        }

        // Phase 2: Now tokens exhausted, should throttle
        let mut throttled_count = 0;
        for _ in 0..10 {
            if rate_limiter.should_throttle() {
                throttled_count += 1;
            }
        }
        assert!(
            throttled_count > 5,
            "Should throttle most requests when no tokens (throttled {})",
            throttled_count
        );

        // Phase 3: Wait for refill and try again
        sleep(Duration::from_secs(1)); // Allow full refill (10 tokens)

        // Should accept ~10 requests now
        let mut accepted_count = 0;
        for _ in 0..10 {
            if !rate_limiter.should_throttle() {
                accepted_count += 1;
            }
        }
        assert!(
            accepted_count >= 8,
            "Should accept most requests after refill (accepted {})",
            accepted_count
        );

        assert!(rate_limiter.accepted_request_rate() > 0.0);
        assert!(!rate_limiter.accepted_request_timestamps.is_empty());
    }

    #[test]
    fn test_should_throttle_under_load_with_external_tps() {
        let pid = PIDController::new_static_controller(10.0);
        let mut rate_limiter = create_rate_limiter(12.0, 12.0, 12.0, pid, Duration::from_secs(1));
        rate_limiter.set_external_accepted_request_rate(2.0);

        // Phase 1: Exhaust tokens with burst
        // Start with 12 tokens, consume them all
        for i in 0..12 {
            let should_throttle = rate_limiter.should_throttle();
            assert!(
                !should_throttle,
                "Should accept burst request {} (have tokens)",
                i
            );
        }

        // Phase 2: Now tokens exhausted, should throttle
        let mut throttled_count = 0;
        for _ in 0..10 {
            if rate_limiter.should_throttle() {
                throttled_count += 1;
            }
        }
        assert!(
            throttled_count > 5,
            "Should throttle most requests when no tokens (throttled {})",
            throttled_count
        );

        // Phase 3: Wait for refill and try again
        sleep(Duration::from_secs(1)); // Allow full refill (12 tokens)

        // Should accept ~12 requests now
        let mut accepted_count = 0;
        for _ in 0..12 {
            if !rate_limiter.should_throttle() {
                accepted_count += 1;
            }
        }
        assert!(
            accepted_count >= 10,
            "Should accept most requests after refill (accepted {})",
            accepted_count
        );

        // Total accepted rate includes external
        assert!(rate_limiter.accepted_request_rate() > 0.0);
        assert!(!rate_limiter.accepted_request_timestamps.is_empty());
    }

    #[test]
    fn test_should_throttle_with_pid_adjustment() {
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        for _ in 0..20 {
            rate_limiter.should_throttle();
        }

        sleep(Duration::from_secs(2));

        let old_refill_rate = rate_limiter.refill_rate();
        rate_limiter.should_throttle();
        let new_refill_rate = rate_limiter.refill_rate();

        // PID should adjust refill_rate (not target_rate)
        assert_ne!(new_refill_rate, old_refill_rate);
        assert!((5.0..=15.0).contains(&new_refill_rate));

        // target_rate should remain constant
        assert_eq!(rate_limiter.target_rate(), 10.0);
    }

    #[test]
    fn test_trim_accepted_timestamps() {
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        let now = Instant::now();
        rate_limiter
            .accepted_request_timestamps
            .push_back(now - Duration::from_secs(2));
        rate_limiter
            .accepted_request_timestamps
            .push_back(now - Duration::from_secs(1));

        rate_limiter.trim_accepted_timestamps(now);

        assert_eq!(rate_limiter.accepted_request_timestamps.len(), 1);
    }

    #[test]
    fn test_calculate_local_accepted_rate() {
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        let now = Instant::now();
        rate_limiter
            .accepted_request_timestamps
            .push_back(now - Duration::from_secs(2));
        rate_limiter
            .accepted_request_timestamps
            .push_back(now - Duration::from_secs(1));

        rate_limiter.calculate_local_accepted_rate(now);

        assert!(rate_limiter.local_accepted_request_rate() > 0.0);
    }

    #[test]
    fn test_external_rates() {
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        rate_limiter.set_external_accepted_request_rate(2.0);

        assert_eq!(rate_limiter.external_accepted_request_rate(), 2.0);
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

        rate_limiter.calculate_local_accepted_rate(now);

        assert_eq!(rate_limiter.accepted_request_rate(), 2.0 + (2.0 / 2.0));
    }

    // ===== Enhanced Unit Tests for Rate Limiter Components =====

    #[test]
    fn test_rate_calculation_minimum_duration_threshold() {
        // Verify that minimum duration threshold (1ms) prevents division by tiny numbers
        let pid = create_pid_controller(1.0, 0.1, 0.01, 0.001, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(100.0, 50.0, 150.0, pid, Duration::from_secs(1));

        let now = Instant::now();

        // Add timestamps very close together (< 1ms)
        rate_limiter
            .accepted_request_timestamps
            .push_back(now - Duration::from_micros(500));
        rate_limiter
            .accepted_request_timestamps
            .push_back(now - Duration::from_micros(400));
        rate_limiter
            .accepted_request_timestamps
            .push_back(now - Duration::from_micros(300));

        rate_limiter.calculate_local_accepted_rate(now);

        // With 3 requests and actual duration 0.0005s, without min threshold:
        // rate would be 3 / 0.0005 = 6000 TPS
        // With min threshold of 0.001s (1ms):
        // rate should be 3 / 0.001 = 3000 TPS
        let calculated_rate = rate_limiter.local_accepted_request_rate();

        assert!(
            (calculated_rate - 3000.0).abs() < 100.0,
            "Rate {} should use 1ms minimum duration",
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
            .accepted_request_timestamps
            .push_back(now - Duration::from_secs(5)); // Too old
        rate_limiter
            .accepted_request_timestamps
            .push_back(now - Duration::from_secs(3)); // Too old
        rate_limiter
            .accepted_request_timestamps
            .push_back(now - Duration::from_secs(1)); // Recent
        rate_limiter.accepted_request_timestamps.push_back(now); // Recent

        rate_limiter.trim_accepted_timestamps(now);

        // Only timestamps within last 2 seconds should remain
        assert_eq!(
            rate_limiter.accepted_request_timestamps.len(),
            2,
            "Should trim old timestamps"
        );

        // Verify the remaining ones are the recent ones
        assert!(
            rate_limiter.accepted_request_timestamps[0]
                .elapsed()
                .as_secs()
                <= 2,
            "First remaining timestamp should be recent"
        );
        assert!(
            rate_limiter.accepted_request_timestamps[1]
                .elapsed()
                .as_secs()
                <= 1,
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
            .accepted_request_timestamps
            .push_back(now - Duration::from_secs(1));
        rate_limiter.accepted_request_timestamps.push_back(now);

        // Set external rates
        rate_limiter.set_external_accepted_request_rate(30.0);

        rate_limiter.calculate_local_accepted_rate(now);

        // Total accepted rate should be local + external
        // Local: 2 requests / 1.0s = 2 TPS
        // External: 30 TPS
        // Total: 32 TPS
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

        // Record initial refill rate
        let initial_refill_rate = rate_limiter.refill_rate();

        // Make many rapid calls (much faster than update_interval)
        for _ in 0..20 {
            rate_limiter.should_throttle();
        }

        // Refill rate should not have changed yet (no time passed)
        let refill_rate_after_rapid = rate_limiter.refill_rate();
        assert_eq!(
            refill_rate_after_rapid, initial_refill_rate,
            "Refill rate should not change without time passing"
        );

        // Wait for update interval to pass
        sleep(Duration::from_millis(600));

        // Generate some load to create error
        for _ in 0..10 {
            rate_limiter.should_throttle();
        }

        // Now PID should have updated
        let refill_rate_after_wait = rate_limiter.refill_rate();
        // Refill rate should have changed (PID adjusted)
        // Can't predict exact value, but should be different
        assert!(
            (refill_rate_after_wait - initial_refill_rate).abs() > 0.01,
            "Refill rate should change after update_interval passes"
        );

        // Target rate should remain constant
        assert_eq!(rate_limiter.target_rate(), 100.0);
    }

    // ===== Token Bucket Unit Tests =====

    #[test]
    fn test_token_bucket_refill() {
        // Verify token refill calculation
        let pid = PIDController::new_static_controller(100.0);
        let mut rate_limiter = create_rate_limiter(100.0, 50.0, 150.0, pid, Duration::from_secs(1));

        // Consume all tokens
        for _ in 0..100 {
            rate_limiter.should_throttle();
        }

        // Should have ~0 tokens now
        assert!(rate_limiter.tokens() < 1.0, "Should have exhausted tokens");

        // Wait 1 second
        sleep(Duration::from_secs(1));

        // Trigger refill by calling should_throttle
        rate_limiter.should_throttle();

        // Should have refilled ~100 tokens (refill_rate=100, elapsed=1.0s)
        let tokens = rate_limiter.tokens();
        assert!(
            tokens >= 99.0,
            "Should refill to ~100 tokens, got {}",
            tokens
        );
    }

    #[test]
    fn test_token_bucket_capacity_limit() {
        // Verify tokens never exceed bucket_capacity
        let pid = PIDController::new_static_controller(100.0);
        let mut rate_limiter = create_rate_limiter(100.0, 50.0, 150.0, pid, Duration::from_secs(1));

        // Start with full bucket (100 tokens)
        assert_eq!(rate_limiter.tokens(), 100.0);

        // Wait a long time
        sleep(Duration::from_secs(2));

        // Trigger refill
        rate_limiter.should_throttle();

        // Should still be capped at bucket_capacity (100)
        let tokens = rate_limiter.tokens();
        assert!(
            tokens <= 100.0,
            "Tokens {} should be capped at capacity 100",
            tokens
        );
    }

    #[test]
    fn test_token_consumption() {
        // Verify tokens decrease by 1 on each accept
        let pid = PIDController::new_static_controller(100.0);
        let mut rate_limiter = create_rate_limiter(100.0, 50.0, 150.0, pid, Duration::from_secs(1));

        let initial_tokens = rate_limiter.tokens();

        // Accept one request
        let throttled = rate_limiter.should_throttle();
        assert!(!throttled, "Should accept request");

        // Tokens should have decreased by 1
        let after_tokens = rate_limiter.tokens();
        assert!(
            (initial_tokens - after_tokens - 1.0).abs() < 0.01,
            "Should consume 1 token, went from {} to {}",
            initial_tokens,
            after_tokens
        );
    }

    #[test]
    fn test_throttle_when_no_tokens() {
        // Verify throttling when tokens < 1
        let pid = PIDController::new_static_controller(10.0);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        // Exhaust all tokens
        for _ in 0..10 {
            rate_limiter.should_throttle();
        }

        // Should have < 1 token
        assert!(
            rate_limiter.tokens() < 1.0,
            "Should have < 1 token, got {}",
            rate_limiter.tokens()
        );

        // Should throttle
        let throttled = rate_limiter.should_throttle();
        assert!(throttled, "Should throttle when tokens < 1");

        // No timestamp should be recorded
        let timestamps_len = rate_limiter.accepted_request_timestamps.len();
        assert_eq!(
            timestamps_len, 10,
            "Throttled request should not record timestamp"
        );
    }

    #[test]
    fn test_refill_rate_adjustment_by_pid() {
        // Verify PID adjusts refill_rate correctly
        let pid = create_pid_controller(100.0, 1.0, 0.1, 0.01, 0.0, None, None);
        let mut rate_limiter = create_rate_limiter(100.0, 50.0, 150.0, pid, Duration::from_secs(1));

        let initial_refill_rate = rate_limiter.refill_rate();
        assert_eq!(initial_refill_rate, 100.0);

        // Generate high load to create negative error
        for _ in 0..150 {
            rate_limiter.should_throttle();
        }

        // Wait for PID update
        sleep(Duration::from_secs(2));
        rate_limiter.should_throttle();

        // PID should have adjusted refill_rate
        let new_refill_rate = rate_limiter.refill_rate();
        assert_ne!(
            new_refill_rate, initial_refill_rate,
            "PID should adjust refill_rate"
        );

        // Should be clamped to [min_rate, max_rate]
        assert!(
            (50.0..=150.0).contains(&new_refill_rate),
            "Refill rate {} should be within bounds [50, 150]",
            new_refill_rate
        );

        // Target rate should remain constant
        assert_eq!(rate_limiter.target_rate(), 100.0);
    }

    #[test]
    fn test_throttle_decision_threshold() {
        // Verify throttling decision is based on token availability
        let pid = PIDController::new_static_controller(10.0);
        let mut rate_limiter = create_rate_limiter(10.0, 5.0, 15.0, pid, Duration::from_secs(1));

        // At start, tokens = 10 (bucket_capacity), should accept up to 10 requests immediately
        for i in 0..10 {
            let throttled = rate_limiter.should_throttle();
            assert!(
                !throttled,
                "Should accept request {} when tokens available",
                i
            );
        }

        // Now tokens are exhausted, should throttle
        let should_throttle = rate_limiter.should_throttle();
        assert!(should_throttle, "Should throttle when no tokens available");

        // Wait for tokens to refill (10 TPS = 100ms per token)
        sleep(Duration::from_millis(500)); // Should refill ~5 tokens

        // Should accept some requests now
        for i in 0..5 {
            let throttled = rate_limiter.should_throttle();
            assert!(!throttled, "Should accept request {} after refill", i);
        }

        // Should throttle again
        let should_throttle = rate_limiter.should_throttle();
        assert!(
            should_throttle,
            "Should throttle again when tokens exhausted"
        );
    }
}
