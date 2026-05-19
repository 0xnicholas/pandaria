use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_core::context::ToolCallCtx;
use agent_core::mutations::HookDecision;
use extensions::builtins::rate_limit::RateLimitExtension;
use extensions::host::extension::Extension;

/// Many tenants hammer the limiter concurrently.
/// Each tenant must get exactly its fair share.
#[tokio::test]
async fn concurrent_multi_tenant_independent_quota() {
    let rate_limit = Arc::new(RateLimitExtension::new(50));
    let tenant_count = 10;
    let calls_per_tenant = 100;

    let mut handles = vec![];
    for tenant_idx in 0..tenant_count {
        let ext = rate_limit.clone();
        let handle = tokio::spawn(async move {
            let mut ctx = ToolCallCtx::new(format!("tenant-{tenant_idx}"), "s1", "test_tool", "call_1");
            ctx.input = serde_json::json!({});

            let mut allowed = 0;
            let mut blocked = 0;
            for _ in 0..calls_per_tenant {
                match ext.on_tool_call(&ctx).await.0 {
                    HookDecision::Continue => allowed += 1,
                    HookDecision::Block { .. } => blocked += 1,
                }
            }
            (allowed, blocked)
        });
        handles.push(handle);
    }

    for (idx, handle) in handles.into_iter().enumerate() {
        let (allowed, blocked) = handle.await.unwrap();
        assert_eq!(
            allowed, 50,
            "tenant-{idx} should have exactly 50 allowed calls, got {allowed}"
        );
        assert_eq!(
            blocked, 50,
            "tenant-{idx} should have exactly 50 blocked calls, got {blocked}"
        );
    }
}

/// A single tenant with many concurrent callers must never exceed its budget.
#[tokio::test]
async fn concurrent_single_tenant_race() {
    let rate_limit = Arc::new(RateLimitExtension::new(100));
    let caller_count = 20;
    let calls_per_caller = 20;

    let allowed_total = Arc::new(AtomicUsize::new(0));
    let blocked_total = Arc::new(AtomicUsize::new(0));

    let mut handles = vec![];
    for _ in 0..caller_count {
        let ext = rate_limit.clone();
        let allowed = allowed_total.clone();
        let blocked = blocked_total.clone();

        let handle = tokio::spawn(async move {
            let mut ctx = ToolCallCtx::new("tenant-0", "s1", "test_tool", "call_1");
            ctx.input = serde_json::json!({});

            for _ in 0..calls_per_caller {
                match ext.on_tool_call(&ctx).await.0 {
                    HookDecision::Continue => {
                        allowed.fetch_add(1, Ordering::SeqCst);
                    }
                    HookDecision::Block { .. } => {
                        blocked.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }

    let allowed = allowed_total.load(Ordering::SeqCst);
    let blocked = blocked_total.load(Ordering::SeqCst);

    assert_eq!(
        allowed, 100,
        "exactly 100 calls should be allowed, got {allowed}"
    );
    assert_eq!(
        blocked,
        caller_count * calls_per_caller - 100,
        "remaining calls should be blocked, got {blocked}"
    );
}

/// Mix of tenants: some within budget, some over.
#[tokio::test]
async fn concurrent_mixed_tenants() {
    let rate_limit = Arc::new(RateLimitExtension::new(10));

    // tenant-0 and tenant-1 will exhaust their budget
    // tenant-2, tenant-3, tenant-4 stay well within budget
    let configs = vec![
        ("tenant-0", 15, 10, 5),  // 15 calls, 10 allowed, 5 blocked
        ("tenant-1", 20, 10, 10), // 20 calls, 10 allowed, 10 blocked
        ("tenant-2", 5, 5, 0),    // 5 calls, all allowed
        ("tenant-3", 3, 3, 0),    // 3 calls, all allowed
        ("tenant-4", 7, 7, 0),    // 7 calls, all allowed
    ];

    let mut handles = vec![];
    for (tenant, total_calls, expected_allowed, expected_blocked) in configs {
        let ext = rate_limit.clone();
        let handle = tokio::spawn(async move {
            let mut ctx = ToolCallCtx::new(tenant, "s1", "test_tool", "call_1");
            ctx.input = serde_json::json!({});

            let mut allowed = 0;
            let mut blocked = 0;
            for _ in 0..total_calls {
                match ext.on_tool_call(&ctx).await.0 {
                    HookDecision::Continue => allowed += 1,
                    HookDecision::Block { .. } => blocked += 1,
                }
            }
            assert_eq!(
                allowed, expected_allowed,
                "{tenant}: expected {expected_allowed} allowed, got {allowed}"
            );
            assert_eq!(
                blocked, expected_blocked,
                "{tenant}: expected {expected_blocked} blocked, got {blocked}"
            );
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }
}

/// Verify that `max_tracked_tenants` is enforced even under concurrent creation.
#[tokio::test]
async fn max_tracked_tenants_enforced_concurrently() {
    let rate_limit = Arc::new(RateLimitExtension::with_config(
        10,
        std::time::Duration::from_secs(60),
        3,
    ));

    // Spawn 10 tasks, each trying to create a new tenant concurrently.
    let mut handles = vec![];
    for i in 0..10 {
        let ext = rate_limit.clone();
        let handle = tokio::spawn(async move {
            let mut ctx = ToolCallCtx::new(format!("tenant-{i}"), "s1", "test_tool", "call_1");
            ctx.input = serde_json::json!({});
            ext.on_tool_call(&ctx).await.0
        });
        handles.push(handle);
    }

    let mut allowed_count = 0;
    let mut blocked_count = 0;
    for h in handles {
        match h.await.unwrap() {
            HookDecision::Continue => allowed_count += 1,
            HookDecision::Block { reason } => {
                assert!(
                    reason.contains("tenant quota exceeded"),
                    "unexpected block reason: {reason}"
                );
                blocked_count += 1;
            }
        }
    }

    // Exactly 3 tenants should be allowed, the rest blocked.
    assert_eq!(allowed_count, 3, "expected 3 allowed tenants, got {allowed_count}");
    assert_eq!(blocked_count, 7, "expected 7 blocked tenants, got {blocked_count}");
}
