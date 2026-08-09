//! A process-wide safety valve for loopback gateway traffic.

use governor::{
    clock::{Clock, DefaultClock},
    middleware::NoOpMiddleware,
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
///
/// The clock is generic so tests can inject a fake clock (`FakeRelativeClock`)
/// instead of depending on the wall clock (which makes quota-recovery tests
/// flaky when CI pauses longer than a minute).
pub struct GatewayRateLimiter<C: Clock = DefaultClock> {
    /// Combined gateway traffic limit (300 requests per minute).
    global_limiter: Arc<RateLimiter<NotKeyed, InMemoryState, C, NoOpMiddleware<C::Instant>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitError {
    GlobalLimit,
}

impl<C: Clock> GatewayRateLimiter<C> {
    /// Create the loopback gateway safety-valve limiter backed by `clock`.
    fn with_clock(clock: C) -> Self {
        Self {
            global_limiter: Arc::new(RateLimiter::direct_with_clock(
                Quota::per_minute(NonZeroU32::new(GLOBAL_REQUESTS_PER_MINUTE).unwrap()),
                &clock,
            )),
        }
    }

    /// Check whether another loopback gateway request may be forwarded.
    pub fn check(&self) -> Result<(), RateLimitError> {
        self.global_limiter
            .check()
            .map_err(|_| RateLimitError::GlobalLimit)
    }
}

impl GatewayRateLimiter<DefaultClock> {
    /// Create the loopback gateway safety-valve limiter.
    pub fn new() -> Self {
        Self::with_clock(DefaultClock::default())
    }
}

impl<C: Clock + Default> Default for GatewayRateLimiter<C> {
    fn default() -> Self {
        Self::with_clock(C::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use governor::clock::FakeRelativeClock;
    use std::time::Duration;

    #[test]
    fn allows_requests_under_global_limit() {
        let limiter = GatewayRateLimiter::new();
        assert!(limiter.check().is_ok());
    }

    #[test]
    fn enforces_global_safety_valve() {
        // 注入 fake 时钟，避免墙钟窗口（慢 CI 暂停超过 1 分钟会使配额部分恢复）。
        let clock = FakeRelativeClock::default();
        let limiter = GatewayRateLimiter::with_clock(clock.clone());
        for _ in 0..GLOBAL_REQUESTS_PER_MINUTE {
            assert!(limiter.check().is_ok());
        }
        assert_eq!(limiter.check(), Err(RateLimitError::GlobalLimit));
    }

    #[test]
    fn quota_recovers_only_over_full_window() {
        let clock = FakeRelativeClock::default();
        let limiter = GatewayRateLimiter::with_clock(clock.clone());
        for _ in 0..GLOBAL_REQUESTS_PER_MINUTE {
            assert!(limiter.check().is_ok());
        }
        assert_eq!(limiter.check(), Err(RateLimitError::GlobalLimit));

        // 完整窗口后恢复全部配额。
        clock.advance(Duration::from_secs(60));
        for _ in 0..GLOBAL_REQUESTS_PER_MINUTE {
            assert!(limiter.check().is_ok());
        }
        assert_eq!(limiter.check(), Err(RateLimitError::GlobalLimit));

        // 半窗口只恢复部分配额：放行一部分，但不会立刻恢复全部。
        clock.advance(Duration::from_secs(30));
        let allowed_half_window = (0..GLOBAL_REQUESTS_PER_MINUTE)
            .take_while(|_| limiter.check().is_ok())
            .count();
        assert!(
            allowed_half_window > 0,
            "half window should restore some quota"
        );
        assert!(
            allowed_half_window < GLOBAL_REQUESTS_PER_MINUTE as usize,
            "half window must not restore the full quota"
        );
    }
}
