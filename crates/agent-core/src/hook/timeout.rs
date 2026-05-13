use std::panic::AssertUnwindSafe;
use std::time::Duration;
use futures::FutureExt;
use tracing::{error, warn};

/// Execute an async block with a timeout, catching panics.
/// Returns the default value if the future panics or does not complete within `timeout_ms`.
///
/// Per ADR constraint: Extension panic must not propagate to the agent loop.
pub async fn with_timeout<F, T>(
    future: F,
    timeout_ms: u64,
    default: T,
    hook_name: &'static str,
) -> T
where
    F: std::future::Future<Output = T>,
{
    let result = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        AssertUnwindSafe(future).catch_unwind(),
    ).await;

    match result {
        Ok(Ok(value)) => value,
        Ok(Err(_)) => {
            error!("Hook '{}' panicked — using default", hook_name);
            default
        }
        Err(_) => {
            warn!(
                "Hook '{}' timed out after {}ms, using default",
                hook_name, timeout_ms
            );
            default
        }
    }
}
