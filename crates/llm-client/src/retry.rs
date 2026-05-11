use std::future::Future;
use std::time::Duration;

use crate::error::LlmError;
use crate::streaming::AssistantMessageEventStream;

/// Retry an LLM request with exponential backoff.
///
/// Retry triggers:
///   - LlmError::RateLimited
///   - LlmError::Overloaded
///   - LlmError::Timeout
///
/// Backoff: 100ms → 200ms → 400ms (2^attempt * 100ms).
/// If max_retry_delay_ms is set and the computed delay exceeds it,
/// the request fails immediately to avoid unreasonably long waits.
///
/// Maximum retry count configurable via `max_retries`.
pub async fn with_retry<F, Fut>(
    operation: F,
    max_retries: u32,
    max_retry_delay_ms: Option<u64>,
) -> Result<AssistantMessageEventStream, LlmError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<AssistantMessageEventStream, LlmError>>,
{
    let mut attempt: u32 = 0;

    loop {
        match operation().await {
            Ok(stream) => {
                if attempt > 0 {
                    tracing::Span::current().record("retry_count", attempt);
                    tracing::info!(retry_count = attempt, "LLM request succeeded after retries");
                }
                return Ok(stream);
            }
            Err(e) if attempt < max_retries && e.is_retryable() => {
                tracing::Span::current().record("retry_count", attempt + 1);
                let delay = Duration::from_millis(100 * 2u64.pow(attempt));
                if let Some(max_delay) = max_retry_delay_ms
                    && delay.as_millis() as u64 > max_delay
                {
                    tracing::warn!(
                        retry_count = attempt + 1,
                        delay_ms = delay.as_millis(),
                        max_retry_delay_ms = max_delay,
                        "retry delay exceeds max, failing request"
                    );
                    return Err(e);
                }
                tracing::warn!(
                    retry_count = attempt + 1,
                    delay_ms = delay.as_millis(),
                    error = %e,
                    "retrying LLM request"
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_retry_success_after_rate_limit() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let result = with_retry(
            move || {
                let c = c.clone();
                async move {
                    let count = c.fetch_add(1, Ordering::SeqCst);
                    if count < 3 {
                        Err(LlmError::RateLimited("try again".to_string()))
                    } else {
                        // Return a mock stream
                        let (stream, _tx) = AssistantMessageEventStream::new(1);
                        Ok(stream)
                    }
                }
            },
            3,
            None,
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 4); // 3 fails + 1 success
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let result = with_retry(
            move || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(LlmError::RateLimited("try again".to_string()))
                }
            },
            2,
            None,
        )
        .await;
        assert!(matches!(result, Err(LlmError::RateLimited(_))));
        assert_eq!(counter.load(Ordering::SeqCst), 3); // initial + 2 retries
    }

    #[tokio::test]
    async fn test_no_retry_on_invalid_request() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let result = with_retry(
            move || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(LlmError::InvalidRequest("bad".to_string()))
                }
            },
            3,
            None,
        )
        .await;
        assert!(matches!(result, Err(LlmError::InvalidRequest(_))));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_no_retry_on_auth_error() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let result = with_retry(
            move || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(LlmError::AuthError("bad key".to_string()))
                }
            },
            3,
            None,
        )
        .await;
        assert!(matches!(result, Err(LlmError::AuthError(_))));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_max_retry_delay_caps() {
        let result = with_retry(
            || async { Err(LlmError::RateLimited("try again".to_string())) },
            5,
            Some(1), // cap at 1ms — first delay is 100ms, exceeds cap
        )
        .await;
        assert!(matches!(result, Err(LlmError::RateLimited(_))));
    }
}
