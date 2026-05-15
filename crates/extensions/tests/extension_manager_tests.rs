use std::sync::Arc;

use agent_core::context::ToolCallCtx;
use agent_core::mutations::HookDecision;
use agent_core::HookDispatcher;
use extensions::builtins::audit::AuditExtension;
use extensions::builtins::rate_limit::RateLimitExtension;
use extensions::builtins::tool_guard::ToolGuardExtension;
use extensions::host::manager::ExtensionManager;

#[tokio::test]
async fn test_manager_collect_tools_dedup() {
    let manager = ExtensionManager::new(vec![]);
    let tools = manager.collect_tools();
    assert!(tools.is_empty());
}

#[tokio::test]
async fn test_manager_spawn_all_creates_router() {
    let manager = ExtensionManager::new(vec![
        Arc::new(AuditExtension),
        Arc::new(RateLimitExtension::new(10)),
    ]);

    let (hook_router, handles, _join_handles) = manager.spawn_all();

    // Router should work — test with a tool call
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, _mutation) = hook_router.on_tool_call(&ctx).await;
    // Audit never blocks, rate-limit under budget → Continue
    assert!(matches!(decision, HookDecision::Continue));

    // Should have 2 handles (one per extension)
    assert_eq!(handles.len(), 2);

    // Clean up
    ExtensionManager::shutdown_all(&handles).await;
}

#[tokio::test]
async fn test_manager_tool_guard_blocks() {
    let manager = ExtensionManager::new(vec![
        Arc::new(ToolGuardExtension::new(
            vec![],
            vec!["forbidden".to_string()],
        )),
    ]);

    let (hook_router, handles, _join_handles) = manager.spawn_all();

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "forbidden".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, _mutation) = hook_router.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));

    ExtensionManager::shutdown_all(&handles).await;
}

#[tokio::test]
async fn test_manager_shutdown_terminates_actors() {
    let manager = ExtensionManager::new(vec![
        Arc::new(AuditExtension),
    ]);

    let (_hook_router, handles, join_handles) = manager.spawn_all();

    // Shutdown
    ExtensionManager::shutdown_all(&handles).await;

    // All join handles should resolve
    for jh in join_handles {
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), jh).await;
        assert!(result.is_ok(), "actor should terminate after shutdown");
    }
}
