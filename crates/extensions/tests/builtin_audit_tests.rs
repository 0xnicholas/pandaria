use agent_core::context::{ToolCallCtx, ToolResultCtx, TurnEndCtx};
use agent_core::mutations::HookDecision;
use extensions::builtins::audit::AuditExtension;
use extensions::host::extension::Extension;

#[tokio::test]
async fn test_audit_on_tool_call_returns_continue() {
    let audit = AuditExtension;
    let mut ctx = ToolCallCtx::new("t1", "s1", "test_tool", "call_1");
    ctx.input = serde_json::json!({});

    let (decision, mutation) = audit.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));
    assert!(mutation.input.is_none());
}

#[tokio::test]
async fn test_audit_on_tool_result_returns_default() {
    let audit = AuditExtension;
    let mut ctx = ToolResultCtx::new("t1", "s1", "test_tool", "call_1");
    ctx.input = serde_json::json!({});

    let mutation = audit.on_tool_result(&ctx).await;
    assert!(mutation.content.is_none());
    assert!(mutation.details.is_none());
    assert!(mutation.is_error.is_none());
    assert!(mutation.terminate.is_none());
}

#[tokio::test]
async fn test_audit_on_turn_end_does_not_panic() {
    let audit = AuditExtension;
    let ctx = TurnEndCtx::new("t1", "s1", 0, ai_provider::Usage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });

    // Should not panic — observational hook is fire-and-forget
    audit.on_turn_end(&ctx).await;
}
