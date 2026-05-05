use std::sync::Arc;

use agent_core::context::{ToolCallCtx, ToolResultCtx, TurnEndCtx};
use agent_core::mutations::{HookDecision, ToolCallMutation};
use extensions::builtins::audit::AuditExtension;
use extensions::host::extension::Extension;

#[tokio::test]
async fn test_audit_on_tool_call_returns_continue() {
    let audit = AuditExtension;
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, mutation) = audit.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));
    assert!(mutation.input.is_none());
}

#[tokio::test]
async fn test_audit_on_tool_result_returns_default() {
    let audit = AuditExtension;
    let ctx = ToolResultCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
        content: vec![],
        details: None,
        is_error: false,
    };

    let mutation = audit.on_tool_result(&ctx).await;
    assert!(mutation.content.is_none());
    assert!(mutation.details.is_none());
    assert!(mutation.is_error.is_none());
    assert!(mutation.terminate.is_none());
}

#[tokio::test]
async fn test_audit_on_turn_end_does_not_panic() {
    let audit = AuditExtension;
    let ctx = TurnEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        turn_index: 0,
        messages: vec![],
    };

    // Should not panic — observational hook is fire-and-forget
    audit.on_turn_end(&ctx).await;
}
