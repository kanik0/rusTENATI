use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::Semaphore;

/// Adaptive concurrency controller using AIMD (Additive Increase, Multiplicative Decrease).
///
/// Automatically adjusts concurrency based on success/failure feedback:
/// - On success: increase by 1 every `increase_interval` successes (up to max)
/// - On 429/5xx: halve concurrency (down to min)
pub struct AdaptiveConcurrency {
    semaphore: Arc<Semaphore>,
    current: AtomicU32,
    min: u32,
    max: u32,
    success_count: AtomicU64,
    increase_interval: u64,
}

impl AdaptiveConcurrency {
    pub fn new(initial: u32, min: u32, max: u32) -> Self {
        let initial = initial.clamp(min, max);
        Self {
            semaphore: Arc::new(Semaphore::new(initial as usize)),
            current: AtomicU32::new(initial),
            min,
            max,
            success_count: AtomicU64::new(0),
            increase_interval: 10,
        }
    }

    /// Get the inner semaphore for acquiring permits.
    pub fn semaphore(&self) -> &Arc<Semaphore> {
        &self.semaphore
    }

    /// Report a successful download. May increase concurrency.
    pub fn report_success(&self) {
        let count = self.success_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count % self.increase_interval == 0 {
            let current = self.current.load(Ordering::Relaxed);
            if current < self.max {
                let new = current + 1;
                self.current.store(new, Ordering::Relaxed);
                self.semaphore.add_permits(1);
                tracing::debug!("Adaptive concurrency increased to {new}");
            }
        }
    }

    /// Report a rate-limited or server error. Halves concurrency.
    pub fn report_throttle(&self) {
        let current = self.current.load(Ordering::Relaxed);
        let new = (current / 2).max(self.min);
        if new < current {
            let decrease = (current - new) as usize;
            self.current.store(new, Ordering::Relaxed);
            // Forget permits to reduce concurrency
            // (acquire_many would block, so we just note the target)
            self.semaphore.forget_permits(decrease);
            tracing::warn!("Adaptive concurrency decreased to {new} (was {current})");
        }
    }

    /// Get current concurrency level.
    pub fn current(&self) -> u32 {
        self.current.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaptive_increase() {
        let ac = AdaptiveConcurrency::new(4, 2, 16);
        assert_eq!(ac.current(), 4);

        // 10 successes should increase by 1
        for _ in 0..10 {
            ac.report_success();
        }
        assert_eq!(ac.current(), 5);
    }

    #[test]
    fn test_adaptive_decrease() {
        let ac = AdaptiveConcurrency::new(8, 2, 16);
        ac.report_throttle();
        assert_eq!(ac.current(), 4);
        ac.report_throttle();
        assert_eq!(ac.current(), 2);
        // Should not go below min
        ac.report_throttle();
        assert_eq!(ac.current(), 2);
    }

    #[test]
    fn test_adaptive_bounds() {
        let ac = AdaptiveConcurrency::new(15, 2, 16);
        // Fill up to max
        for _ in 0..20 {
            ac.report_success();
        }
        assert_eq!(ac.current(), 16); // capped at max
    }
}
