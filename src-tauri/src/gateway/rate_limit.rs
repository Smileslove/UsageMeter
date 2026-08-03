//! A process-wide safety valve for loopback gateway traffic.

use governor::{
    clock::DefaultClock,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter,
};
use std::num::NonZeroU32;
use std::sync::Arc;

/// Maximum combined gateway throughput. This is deliberately a generous
/// backstop for a single local user, rather than a quota for normal usage.
const GLOBAL_REQUESTS_PER_MINUTE: u32 = 300;

/// Rate limiter for all gateway traffic.
///
/// The listener is bound exclusively to loopback. A single global limiter is
/// therefore sufficient as a defense-in-depth control against a runaway local
/// process or a leaked local key, without constraining legitimate concurrent
/// client tools with separate per-key buckets.
pub struct GatewayRateLimiter {
    /// Combined gateway traffic limit (300 requests per minute).
    global_limiter: Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitError {
    GlobalLimit,
}

impl GatewayRateLimiter {
    /// Create the loopback gateway safety-valve limiter.
    pub fn new() -> Self {
        Self {
            global_limiter: Arc::new(RateLimiter::direct(Quota::per_minute(
                NonZeroU32::new(GLOBAL_REQUESTS_PER_MINUTE).unwrap(),
            ))),
        }
    }

    /// Check whether another loopback gateway request may be forwarded.
    pub fn check(&self) -> Result<(), RateLimitError> {
        self.global_limiter
            .check()
            .map_err(|_| RateLimitError::GlobalLimit)
    }
}

impl Default for GatewayRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_requests_under_global_limit() {
        let limiter = GatewayRateLimiter::new();
        assert!(limiter.check().is_ok());
    }

    #[test]
    fn enforces_global_safety_valve() {
        let limiter = GatewayRateLimiter::new();
        for _ in 0..GLOBAL_REQUESTS_PER_MINUTE {
            assert!(limiter.check().is_ok());
        }
        assert_eq!(limiter.check(), Err(RateLimitError::GlobalLimit));
    }
}
