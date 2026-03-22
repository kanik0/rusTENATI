use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};

type InnerLimiter = Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>;

/// Per-host rate limiter that maintains separate rate limits for each hostname.
/// This prevents one slow/throttled host from consuming the entire rate budget.
#[derive(Clone)]
pub struct PerHostLimiter {
    limiters: Arc<Mutex<HashMap<String, InnerLimiter>>>,
    per_second: u32,
}

impl PerHostLimiter {
    /// Create a new per-host rate limiter with the given per-second rate per host.
    pub fn new(per_second: u32) -> Self {
        Self {
            limiters: Arc::new(Mutex::new(HashMap::new())),
            per_second: per_second.max(1),
        }
    }

    /// Wait until a request to the given URL is allowed.
    pub async fn until_ready(&self, url: &str) {
        let host = extract_host(url);
        let limiter = {
            let mut map = self.limiters.lock().unwrap();
            map.entry(host)
                .or_insert_with(|| {
                    let quota = Quota::per_second(NonZeroU32::new(self.per_second).unwrap());
                    Arc::new(RateLimiter::direct(quota))
                })
                .clone()
        };
        limiter.until_ready().await;
    }
}

fn extract_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}
