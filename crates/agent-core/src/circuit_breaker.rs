//! Circuit breaker for LLM provider calls.
//!
//! Prevents cascading failures by fast-failing requests when a provider
//! has recently experienced consecutive errors.
//!
//! # State machine
//!
//! ```text
//! Closed ──[failures >= threshold]──► Open ──[timeout expires]──► HalfOpen
//!   ▲                                    │                         │
//!   │                                    │                         │
//!   └──────────[success]─────────────────┴────────[success]────────┘
//! ```

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::RwLock;

/// Per-provider circuit breaker.
///
/// Default thresholds:
/// - failure threshold: 5 errors in a row
/// - open duration: 30 seconds
/// - half-open max requests: 1
#[derive(Debug)]
pub struct CircuitBreaker {
    failure_threshold: u32,
    open_duration: Duration,
    half_open_max: u32,

    state: RwLock<State>,
    consecutive_failures: AtomicU32,
    half_open_requests: AtomicU32,
    last_failure_time: AtomicU64, // epoch millis
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Closed,
    Open,
    HalfOpen,
}

impl CircuitBreaker {
    /// Create a circuit breaker with default thresholds.
    pub fn new() -> Self {
        Self::with_config(5, Duration::from_secs(30), 1)
    }

    /// Create with custom thresholds.
    pub fn with_config(failure_threshold: u32, open_duration: Duration, half_open_max: u32) -> Self {
        Self {
            failure_threshold,
            open_duration,
            half_open_max,
            state: RwLock::new(State::Closed),
            consecutive_failures: AtomicU32::new(0),
            half_open_requests: AtomicU32::new(0),
            last_failure_time: AtomicU64::new(0),
        }
    }

    /// Check whether the circuit allows a request through.
    ///
    /// Returns `Ok(())` if the request should proceed.
    /// Returns `Err` with the remaining cooldown if the circuit is Open.
    pub async fn check(&self) -> Result<(), CircuitBreakerError> {
        // Fast path: read lock for Closed and HalfOpen
        {
            let state = self.state.read().await;
            match *state {
                State::Closed => return Ok(()),
                State::HalfOpen => {
                    let current = self.half_open_requests.fetch_add(1, Ordering::SeqCst);
                    if current >= self.half_open_max {
                        self.half_open_requests.fetch_sub(1, Ordering::SeqCst);
                        return Err(CircuitBreakerError::Open {
                            remaining: self.open_duration,
                        });
                    }
                    return Ok(());
                }
                State::Open => {} // fall through to slow path
            }
        }

        // Slow path: Open state may transition to HalfOpen
        let mut state = self.state.write().await;
        match *state {
            State::Open => {
                let last_failure = self.load_last_failure_time();
                let elapsed = Instant::now().duration_since(last_failure);
                if elapsed >= self.open_duration {
                    *state = State::HalfOpen;
                    self.half_open_requests.store(0, Ordering::SeqCst);
                    // Now behave like HalfOpen
                    drop(state);
                    let current = self.half_open_requests.fetch_add(1, Ordering::SeqCst);
                    if current >= self.half_open_max {
                        self.half_open_requests.fetch_sub(1, Ordering::SeqCst);
                        Err(CircuitBreakerError::Open {
                            remaining: self.open_duration,
                        })
                    } else {
                        Ok(())
                    }
                } else {
                    let remaining = self.open_duration.saturating_sub(elapsed);
                    Err(CircuitBreakerError::Open { remaining })
                }
            }
            // Another thread may have transitioned the state while we waited for write lock
            State::Closed => Ok(()),
            State::HalfOpen => {
                let current = self.half_open_requests.fetch_add(1, Ordering::SeqCst);
                if current >= self.half_open_max {
                    self.half_open_requests.fetch_sub(1, Ordering::SeqCst);
                    Err(CircuitBreakerError::Open {
                        remaining: self.open_duration,
                    })
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Record a successful request.
    pub async fn record_success(&self) {
        let mut state = self.state.write().await;
        self.consecutive_failures.store(0, Ordering::SeqCst);
        match *state {
            State::HalfOpen => {
                *state = State::Closed;
                self.half_open_requests.store(0, Ordering::SeqCst);
            }
            _ => {}
        }
    }

    /// Record a failed request.
    pub async fn record_failure(&self) {
        let mut state = self.state.write().await;
        let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        self.store_last_failure_time(Instant::now());

        match *state {
            State::Closed if failures >= self.failure_threshold => {
                *state = State::Open;
                tracing::warn!(
                    failures = failures,
                    threshold = self.failure_threshold,
                    "circuit breaker opened"
                );
            }
            State::HalfOpen => {
                *state = State::Open;
                self.half_open_requests.fetch_sub(1, Ordering::SeqCst);
                tracing::warn!("circuit breaker reopened from half-open");
            }
            _ => {}
        }
    }

    fn load_last_failure_time(&self) -> Instant {
        let millis = self.last_failure_time.load(Ordering::SeqCst);
        if millis == 0 {
            Instant::now() - Duration::from_secs(3600)
        } else {
            let stored = UNIX_EPOCH + Duration::from_millis(millis);
            match SystemTime::now().duration_since(stored) {
                Ok(elapsed) => Instant::now().checked_sub(elapsed)
                    .unwrap_or_else(|| Instant::now() - Duration::from_secs(3600)),
                Err(_) => Instant::now() - Duration::from_secs(3600), // clock moved backward
            }
        }
    }

    fn store_last_failure_time(&self, _instant: Instant) {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_failure_time.store(millis, Ordering::SeqCst);
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

/// Error returned when the circuit breaker is open.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CircuitBreakerError {
    #[error("circuit breaker is open, retry after {remaining:?}")]
    Open { remaining: Duration },
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_closed_allows_requests() {
        let cb = CircuitBreaker::new();
        assert!(cb.check().await.is_ok());
    }

    #[tokio::test]
    async fn test_opens_after_threshold() {
        let cb = CircuitBreaker::with_config(3, Duration::from_secs(60), 1);

        for _ in 0..3 {
            cb.record_failure().await;
        }

        assert!(cb.check().await.is_err());
    }

    #[tokio::test]
    async fn test_success_resets_counter() {
        let cb = CircuitBreaker::with_config(3, Duration::from_secs(60), 1);

        cb.record_failure().await;
        cb.record_failure().await;
        cb.record_success().await;
        cb.record_failure().await;

        // After success, counter is reset, so 1 more failure should still be ok
        assert!(cb.check().await.is_ok());
    }

    #[tokio::test]
    async fn test_half_open_on_timeout() {
        let cb = CircuitBreaker::with_config(1, Duration::from_millis(50), 1);

        cb.record_failure().await;
        assert!(cb.check().await.is_err());

        // Wait for open duration to expire
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Should transition to HalfOpen
        assert!(cb.check().await.is_ok());
    }

    #[tokio::test]
    async fn test_half_open_success_closes() {
        let cb = CircuitBreaker::with_config(1, Duration::from_millis(50), 1);

        cb.record_failure().await;
        tokio::time::sleep(Duration::from_millis(60)).await;

        assert!(cb.check().await.is_ok());
        cb.record_success().await;

        // Should be closed now
        assert!(cb.check().await.is_ok());
        assert!(cb.check().await.is_ok());
    }

    #[tokio::test]
    async fn test_half_open_failure_reopens() {
        let cb = CircuitBreaker::with_config(1, Duration::from_millis(50), 1);

        cb.record_failure().await;
        tokio::time::sleep(Duration::from_millis(60)).await;

        assert!(cb.check().await.is_ok());
        cb.record_failure().await;

        // Should reopen
        assert!(cb.check().await.is_err());
    }

    #[tokio::test]
    async fn test_time_storage_does_not_panic_on_restart_simulation() {
        let cb = CircuitBreaker::with_config(1, Duration::from_secs(60), 1);

        // Simulate a stored time from a previous process by storing a large epoch value
        // and then loading it back — should not panic.
        cb.store_last_failure_time(Instant::now());
        let _ = cb.load_last_failure_time();

        // Simulate clock moving backward by storing a future time manually
        let future_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 60_000; // 1 minute in the future
        cb.last_failure_time.store(future_epoch, Ordering::SeqCst);

        // load_last_failure_time should handle this gracefully without panic
        let loaded = cb.load_last_failure_time();
        // Because the stored time is in the future, load_last_failure_time
        // should return approximately Instant::now() - 3600s (clock backward fallback)
        // which is effectively a time in the past, allowing check() to proceed to HalfOpen
        let elapsed = Instant::now().duration_since(loaded);
        assert!(elapsed >= Duration::from_secs(3590));
    }

    #[tokio::test]
    async fn test_concurrent_check_does_not_deadlock() {
        let cb = Arc::new(CircuitBreaker::with_config(100, Duration::from_secs(60), 1));
        let mut handles = vec![];

        for _ in 0..20 {
            let cb = cb.clone();
            handles.push(tokio::spawn(async move {
                cb.check().await
            }));
        }

        let results = futures::future::join_all(handles).await;
        for res in results {
            assert!(res.is_ok());
            assert!(res.unwrap().is_ok());
        }
    }
}
