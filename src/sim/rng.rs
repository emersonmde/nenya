//! Deterministic pseudo-random number generation for reproducible simulations.
//!
//! The simulator's hard requirement is that the same seed produces identical
//! results byte for byte, across runs and forever. A hand-rolled SplitMix64
//! guarantees that: the algorithm is fixed here in ~10 lines, so no external
//! crate version bump can ever change the stream (RNG crates explicitly do
//! not promise value-stability across major versions).

/// SplitMix64 pseudo-random number generator.
///
/// Algorithm and constants from Sebastiano Vigna's public-domain reference
/// implementation (<https://prng.di.unimi.it/splitmix64.c>), based on
/// Steele, Lea & Flood, "Fast Splittable Pseudorandom Number Generators",
/// OOPSLA 2014. Passes BigCrush; more than adequate for workload generation.
#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    /// Next uniformly distributed u64.
    pub fn next_u64(&mut self) -> u64 {
        // Constants from splitmix64.c (see module docs): golden-ratio
        // increment and the two finalization multipliers.
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform f64 in [0, 1), using the top 53 bits (full mantissa precision).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Sample a Poisson-distributed count with the given mean.
    ///
    /// Knuth's multiplication algorithm (TAOCP Vol. 2, §3.4.1). Runtime is
    /// O(lambda), which is fine here: per-tick means are `rate × tick`, a few
    /// tens at most. Large means are split into chunks so `exp(-lambda)`
    /// never underflows.
    pub fn poisson(&mut self, lambda: f64) -> u64 {
        if lambda <= 0.0 {
            return 0;
        }

        let mut remaining = lambda;
        let mut total = 0u64;
        while remaining > 0.0 {
            let chunk = remaining.min(500.0);
            remaining -= chunk;

            let limit = (-chunk).exp();
            let mut product = 1.0;
            loop {
                product *= self.next_f64();
                if product <= limit {
                    break;
                }
                total += 1;
            }
        }
        total
    }

    /// Derive an independent generator (e.g., one stream per node) so that
    /// consuming values in one stream never perturbs another.
    pub fn fork(&mut self) -> SplitMix64 {
        SplitMix64::new(self.next_u64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_reference_vectors() {
        // First three outputs of splitmix64.c for seed 1234567, computed from
        // the reference implementation
        let mut rng = SplitMix64::new(1234567);
        assert_eq!(rng.next_u64(), 6457827717110365317);
        assert_eq!(rng.next_u64(), 3203168211198807973);
        assert_eq!(rng.next_u64(), 9817491932198370423);
    }

    #[test]
    fn test_same_seed_same_stream() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn test_f64_in_unit_interval() {
        let mut rng = SplitMix64::new(7);
        for _ in 0..10_000 {
            let x = rng.next_f64();
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn test_poisson_mean() {
        let mut rng = SplitMix64::new(99);
        let lambda = 4.0;
        let n = 20_000;
        let total: u64 = (0..n).map(|_| rng.poisson(lambda)).sum();
        let mean = total as f64 / n as f64;
        // Standard error is sqrt(lambda/n) ≈ 0.014; 5 sigma bound
        assert!(
            (mean - lambda).abs() < 0.08,
            "Poisson mean {} too far from {}",
            mean,
            lambda
        );
    }

    #[test]
    fn test_poisson_large_lambda_no_underflow() {
        let mut rng = SplitMix64::new(3);
        let sample = rng.poisson(2000.0);
        assert!(sample > 1500 && sample < 2500, "got {}", sample);
    }

    #[test]
    fn test_zero_lambda() {
        let mut rng = SplitMix64::new(5);
        assert_eq!(rng.poisson(0.0), 0);
    }
}
