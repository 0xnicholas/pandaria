use agent_core::context::ToolCallCtx;
use agent_core::mutations::HookDecision;
use extensions::builtins::rate_limit::RateLimitExtension;
use extensions::host::extension::Extension;

#[tokio::test]
async fn test_rate_limit_allows_under_budget() {
    let rate_limit = RateLimitExtension::new(5);
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };

    // First 5 calls should be allowed
    for i in 0..5 {
        let (decision, mutation) = rate_limit.on_tool_call(&ctx).await;
        assert!(
            matches!(decision, HookDecision::Continue),
            "call {} should be allowed",
            i
        );
        assert!(mutation.input.is_none());
    }
}

#[tokio::test]
async fn test_rate_limit_blocks_over_budget() {
    let rate_limit = RateLimitExtension::new(2);
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };

    // First 2 calls allowed
    let _ = rate_limit.on_tool_call(&ctx).await;
    let _ = rate_limit.on_tool_call(&ctx).await;

    // 3rd call should be blocked
    let (decision, mutation) = rate_limit.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
    assert!(mutation.input.is_none());
}

#[tokio::test]
#[ignore = "slow: waits for rate-limit window (61s+)"]
async fn test_rate_limit_resets_after_window() {
    let rate_limit = RateLimitExtension::new(2);
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };

    // Exhaust the budget
    let _ = rate_limit.on_tool_call(&ctx).await;
    let _ = rate_limit.on_tool_call(&ctx).await;
    let (decision, _) = rate_limit.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));

    // Wait for the 60s window to expire
    tokio::time::sleep(std::time::Duration::from_secs(61)).await;

    // Should be allowed again after window resets
    let (decision, _) = rate_limit.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));
}
