use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;

use agent_core::context::ToolCallCtx;
use agent_core::mutations::{HookDecision, ToolCallMutation};

use crate::host::extension::Extension;

const DEFAULT_WINDOW: Duration = Duration::from_secs(60);
const DEFAULT_MAX_TRACKED_TENANTS: usize = 10_000;

/// Rate-limit extension — sliding-window tool call frequency limit per tenant.
///
/// Each tenant gets an independent call budget.  A global cap on the number
/// of tracked tenants prevents unbounded memory growth.
pub struct RateLimitExtension {
    max_calls_per_window: u64,
    window: Duration,
    max_tracked_tenants: usize,
    call_times: DashMap<String, Arc<tokio::sync::Mutex<VecDeque<Instant>>>>,
}

impl RateLimitExtension {
    /// Create a rate limiter with the given calls-per-minute budget.
    ///
    /// Defaults:
    /// - `window` = 60 seconds
    /// - `max_tracked_tenants` = 10_000
    pub fn new(max_calls_per_minute: u64) -> Self {
        Self::with_config(max_calls_per_minute, DEFAULT_WINDOW, DEFAULT_MAX_TRACKED_TENANTS)
    }

    /// Create a rate limiter with full control over the window and tenant cap.
    pub fn with_config(
        max_calls_per_window: u64,
        window: Duration,
        max_tracked_tenants: usize,
    ) -> Self {
        Self {
            max_calls_per_window,
            window,
            max_tracked_tenants,
            call_times: DashMap::new(),
        }
    }

    /// Remove tenants whose sliding-window queues are empty (after pruning
    /// expired timestamps).
    fn evict_idle_tenants(&self) {
        let now = Instant::now();
        self.call_times
            .retain(|_tenant_id, tenant_lock| match tenant_lock.try_lock() {
                Ok(mut times) => {
                    while let Some(front) = times.front() {
                        if now.saturating_duration_since(*front) >= self.window {
                            times.pop_front();
                        } else {
                            break;
                        }
                    }
                    !times.is_empty()
                }
                Err(_) => true,
            });
    }
}

#[async_trait]
impl Extension for RateLimitExtension {
    fn name(&self) -> &str {
        "rate-limit"
    }

    async fn on_tool_call(
        &self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        // 1. Enforce global tenant cap (defence against unbounded memory).
        if self.call_times.len() >= self.max_tracked_tenants
            && !self.call_times.contains_key(&ctx.tenant_id)
        {
            // Evict idle tenants whose sliding windows have emptied before
            // permanently rejecting the new tenant.
            self.evict_idle_tenants();

            if self.call_times.len() >= self.max_tracked_tenants {
                return (
                    HookDecision::Block {
                        reason: format!(
                            "rate limiter tenant quota exceeded (max: {})",
                            self.max_tracked_tenants
                        ),
                    },
                    ToolCallMutation::default(),
                );
            }
        }

        // 2. Get or create the per-tenant lock.
        //    We clone the Arc so we can drop the DashMap shard lock immediately.
        let tenant_lock = self
            .call_times
            .entry(ctx.tenant_id.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(VecDeque::new())))
            .clone();

        // 3. Acquire the tenant-specific async mutex.
        let mut times = tenant_lock.lock().await;
        let now = Instant::now();

        // 4. Prune expired entries from the front (VecDeque gives O(1) pop).
        while let Some(front) = times.front() {
            if now.saturating_duration_since(*front) >= self.window {
                times.pop_front();
            } else {
                break;
            }
        }

        // 5. Check budget.
        if times.len() as u64 >= self.max_calls_per_window {
            return (
                HookDecision::Block {
                    reason: format!(
                        "rate limit exceeded: {} tool calls in {:?} (limit: {})",
                        times.len(),
                        self.window,
                        self.max_calls_per_window
                    ),
                },
                ToolCallMutation::default(),
            );
        }

        // 6. Record the call and allow it through.
        times.push_back(now);
        (HookDecision::Continue, ToolCallMutation::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_vecdeque_prunes_correctly() {
        let ext = RateLimitExtension::with_config(3, Duration::from_millis(100), 10);
        let ctx = ToolCallCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            tool_name: "test".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({}),
        };

        // 3 calls within budget
        assert!(matches!(ext.on_tool_call(&ctx).await.0, HookDecision::Continue));
        assert!(matches!(ext.on_tool_call(&ctx).await.0, HookDecision::Continue));
        assert!(matches!(ext.on_tool_call(&ctx).await.0, HookDecision::Continue));

        // 4th call blocked
        assert!(matches!(ext.on_tool_call(&ctx).await.0, HookDecision::Block { .. }));

        // Wait for window to expire
        tokio::time::sleep(Duration::from_millis(120)).await;

        // After expiry, calls are allowed again
        assert!(matches!(ext.on_tool_call(&ctx).await.0, HookDecision::Continue));
    }

    #[tokio::test]
    async fn test_tenant_cap_blocks_new_tenant() {
        let ext = RateLimitExtension::with_config(10, Duration::from_secs(60), 2);

        // Fill up the tenant cap
        for i in 0..2 {
            let ctx = ToolCallCtx {
                tenant_id: format!("tenant-{i}"),
                session_id: "s1".to_string(),
                tool_name: "test".to_string(),
                tool_call_id: "c1".to_string(),
                input: serde_json::json!({}),
            };
            assert!(matches!(ext.on_tool_call(&ctx).await.0, HookDecision::Continue));
        }

        // 3rd new tenant should be blocked by the cap
        let ctx3 = ToolCallCtx {
            tenant_id: "tenant-2".to_string(),
            session_id: "s1".to_string(),
            tool_name: "test".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({}),
        };
        let (decision, _) = ext.on_tool_call(&ctx3).await;
        assert!(
            matches!(decision, HookDecision::Block { reason } if reason.contains("tenant quota exceeded"))
        );
    }

    #[tokio::test]
    async fn test_idle_tenants_get_evicted() {
        let ext = RateLimitExtension::with_config(10, Duration::from_millis(100), 1);

        let ctx_a = ToolCallCtx {
            tenant_id: "tenant-a".to_string(),
            session_id: "s1".to_string(),
            tool_name: "test".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({}),
        };
        // tenant-a takes the single slot
        assert!(matches!(ext.on_tool_call(&ctx_a).await.0, HookDecision::Continue));

        // Wait for the window to expire so tenant-a's VecDeque becomes empty
        tokio::time::sleep(Duration::from_millis(150)).await;

        let ctx_b = ToolCallCtx {
            tenant_id: "tenant-b".to_string(),
            session_id: "s1".to_string(),
            tool_name: "test".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({}),
        };
        // tenant-b should now succeed because tenant-a was evicted
        assert!(matches!(ext.on_tool_call(&ctx_b).await.0, HookDecision::Continue));
    }
}
