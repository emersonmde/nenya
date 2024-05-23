struct RateLimiter {
}

impl RateLimiter {
    fn new() -> RateLimiter {
        RateLimiter {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let _rate_limiter = RateLimiter::new();
    }
}
