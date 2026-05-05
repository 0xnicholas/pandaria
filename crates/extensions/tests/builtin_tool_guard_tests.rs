use std::sync::Arc;

use agent_core::context::ToolCallCtx;
use agent_core::mutations::{HookDecision, ToolCallMutation};
use extensions::builtins::tool_guard::ToolGuardExtension;
use extensions::host::extension::Extension;

#[tokio::test]
async fn test_tool_guard_denies_forbidden_tool() {
    let guard = ToolGuardExtension::new(
        vec!["safe_tool".to_string()],
        vec!["dangerous_tool".to_string()],
    );
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "dangerous_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, mutation) = guard.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
    assert!(mutation.input.is_none());
}

#[tokio::test]
async fn test_tool_guard_denies_tool_not_in_allowed_list() {
    let guard = ToolGuardExtension::new(
        vec!["safe_tool".to_string()],
        vec![],
    );
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "unknown_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, mutation) = guard.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
    assert!(mutation.input.is_none());
}

#[tokio::test]
async fn test_tool_guard_allows_safe_tool() {
    let guard = ToolGuardExtension::new(
        vec!["safe_tool".to_string()],
        vec!["dangerous_tool".to_string()],
    );
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "safe_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, mutation) = guard.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));
    assert!(mutation.input.is_none());
}

#[tokio::test]
async fn test_tool_guard_allows_all_when_allowed_list_empty() {
    let guard = ToolGuardExtension::new(
        vec![],
        vec!["dangerous_tool".to_string()],
    );
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "any_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };

    // allowed_tools is empty → any tool is allowed (except denied)
    let (decision, mutation) = guard.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));
    assert!(mutation.input.is_none());
}
