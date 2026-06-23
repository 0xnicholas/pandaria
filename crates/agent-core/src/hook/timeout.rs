use futures::FutureExt;
use std::panic::AssertUnwindSafe;
use std::time::Duration;
use tracing::{error, warn};

use super::dispatcher::HookDispatcher;

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
    )
    .await;

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

/// Like [`with_timeout`], but reads the timeout from the dispatcher's
/// [`HookDispatcher::hook_timeout_ms`] method.
///
/// This makes per-hook timeouts configurable end-to-end:
/// `HookConfig::hook_timeout_ms` → `DefaultHookDispatcher` → here, with no
/// hardcoded values at the call sites.
///
/// Accepts `&dyn HookDispatcher` so callers can pass either a concrete
/// dispatcher reference or `&*arc_dispatcher` (the latter is needed because
/// `Arc<dyn HookDispatcher>` doesn't directly implement `HookDispatcher`).
pub async fn with_timeout_from<F, T>(
    dispatcher: &dyn HookDispatcher,
    future: F,
    default: T,
    hook_name: &'static str,
) -> T
where
    F: std::future::Future<Output = T>,
{
    let timeout_ms = dispatcher.hook_timeout_ms();
    with_timeout(future, timeout_ms, default, hook_name).await
}
