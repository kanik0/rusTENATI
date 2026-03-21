use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// Per-domain circuit breaker that pauses requests when failure rate is too high.
///
/// States:
/// - Closed: requests flow normally
/// - Open: requests are blocked for a cooldown period
/// - HalfOpen: one probe request is allowed; success closes, failure reopens
pub struct CircuitBreaker {
    failures: AtomicU32,
    threshold: u32,
    cooldown: Duration,
    state: Mutex<BreakerState>,
    total_trips: AtomicU64,
}

enum BreakerState {
    Closed,
    Open { opened_at: Instant },
    HalfOpen,
}

impl CircuitBreaker {
    pub fn new(threshold: u32, cooldown: Duration) -> Self {
        Self {
            failures: AtomicU32::new(0),
            threshold,
            cooldown,
            state: Mutex::new(BreakerState::Closed),
            total_trips: AtomicU64::new(0),
        }
    }

    /// Check if a request should be allowed.
    /// Returns Ok(()) if allowed, Err with wait duration if circuit is open.
    pub async fn check(&self) -> Result<(), Duration> {
        let mut state = self.state.lock().await;
        match *state {
            BreakerState::Closed => Ok(()),
            BreakerState::Open { opened_at } => {
                let elapsed = opened_at.elapsed();
                if elapsed >= self.cooldown {
                    *state = BreakerState::HalfOpen;
                    tracing::debug!("Circuit breaker: half-open (allowing probe request)");
                    Ok(())
                } else {
                    Err(self.cooldown - elapsed)
                }
            }
            BreakerState::HalfOpen => {
                // Only one probe at a time; others wait
                Err(Duration::from_millis(500))
            }
        }
    }

    /// Report a successful request.
    pub async fn report_success(&self) {
        self.failures.store(0, Ordering::Relaxed);
        let mut state = self.state.lock().await;
        if matches!(*state, BreakerState::HalfOpen) {
            *state = BreakerState::Closed;
            tracing::info!("Circuit breaker: closed (recovered)");
        }
    }

    /// Report a failed request (429 or 5xx).
    pub async fn report_failure(&self) {
        let failures = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
        if failures >= self.threshold {
            let mut state = self.state.lock().await;
            if !matches!(*state, BreakerState::Open { .. }) {
                *state = BreakerState::Open { opened_at: Instant::now() };
                self.total_trips.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    "Circuit breaker: OPEN after {} failures (cooldown: {:?})",
                    failures,
                    self.cooldown
                );
            }
        }
    }

    /// Total number of times the circuit has tripped.
    pub fn total_trips(&self) -> u64 {
        self.total_trips.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_circuit_breaker_trips() {
        let cb = CircuitBreaker::new(3, Duration::from_millis(100));

        // Should be closed initially
        assert!(cb.check().await.is_ok());

        // Report failures up to threshold
        cb.report_failure().await;
        cb.report_failure().await;
        assert!(cb.check().await.is_ok()); // still closed

        cb.report_failure().await; // trips the breaker
        assert!(cb.check().await.is_err()); // now open

        // Wait for cooldown
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(cb.check().await.is_ok()); // half-open

        // Success closes it
        cb.report_success().await;
        assert!(cb.check().await.is_ok()); // closed again
    }
}
