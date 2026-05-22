use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use tracing::error;

use crate::error::AgentError;

/// Execute an async block, catching panics and converting them to AgentError.
///
/// Per ADR constraint: Extension panic must not propagate to the agent loop.
pub async fn catch_panic<F, T>(f: F) -> Result<T, AgentError>
where
    F: std::future::Future<Output = T>,
{
    match AssertUnwindSafe(f).catch_unwind().await {
        Ok(result) => Ok(result),
        Err(_) => {
            error!("Extension or tool panicked — converting to error");
            Err(AgentError::ToolExecutionFailed(
                "Extension or tool panicked".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_catch_panic_converts_panic_to_error() {
        let result = catch_panic(async {
            panic!("deliberate panic");
        })
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(
                err,
                AgentError::ToolExecutionFailed(ref msg) if msg == "Extension or tool panicked"
            ),
            "expected ToolExecutionFailed, got {:?}",
            err
        );
    }

    #[tokio::test]
    async fn test_catch_panic_returns_ok_on_normal_completion() {
        let result = catch_panic(async { 42 }).await;
        assert_eq!(result.unwrap(), 42);
    }
}
