use std::num::NonZeroU32;
use std::sync::Arc;

use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};

pub type Limiter = Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>;

/// Create a rate limiter that allows `per_second` requests per second.
pub fn create_rate_limiter(per_second: u32) -> Limiter {
    let quota = Quota::per_second(NonZeroU32::new(per_second.max(1)).unwrap());
    Arc::new(RateLimiter::direct(quota))
}
