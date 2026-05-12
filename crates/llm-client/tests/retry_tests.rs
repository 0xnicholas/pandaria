use llm_client::{AssistantMessageEventStream, LlmError, with_retry};
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
                Err(LlmError::InvalidRequest("bad request".to_string()))
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
