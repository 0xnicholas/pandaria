use agent_core::context::ToolCallCtx;
use agent_core::hook_dispatcher::HookDispatcher;
use agent_core::mutations::{HookDecision, ToolCallMutation};
use async_trait::async_trait;

struct DefaultDispatcher;
#[async_trait]
impl HookDispatcher for DefaultDispatcher {}

struct BlockingDispatcher;
#[async_trait]
impl HookDispatcher for BlockingDispatcher {
    async fn on_tool_call(&self,
        _ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        (
            HookDecision::Block {
                reason: "blocked by test".to_string(),
            },
            ToolCallMutation::default(),
        )
    }
}

#[tokio::test]
async fn test_default_dispatcher_allows_all() {
    let dispatcher = DefaultDispatcher;
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, _mutation) = dispatcher.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));
}

#[tokio::test]
async fn test_custom_dispatcher_blocks() {
    let dispatcher = BlockingDispatcher;
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, _mutation) = dispatcher.on_tool_call(&ctx).await;
    match decision {
        HookDecision::Block { reason } => {
            assert_eq!(reason, "blocked by test");
        }
        other => panic!("expected Block, got {:?}", other),
    }
}

#[tokio::test]
async fn test_default_hooks_return_defaults() {
    let dispatcher = DefaultDispatcher;
    let ctx = agent_core::context::ToolResultCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
        content: vec![],
        details: None,
        is_error: false,
    };

    let mutation = dispatcher.on_tool_result(&ctx).await;
    assert!(mutation.content.is_none());
    assert!(mutation.details.is_none());
    assert!(mutation.is_error.is_none());

    let ctx = agent_core::context::ContextCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![],
    };

    let mutation = dispatcher.on_context(&ctx).await;
    assert!(mutation.messages.is_none());

    let ctx = agent_core::context::TurnEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        turn_index: 0,
        messages: vec![],
    };

    // Observational hooks should not panic
    dispatcher.on_turn_end(&ctx).await;

    let ctx = agent_core::context::AgentEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![],
    };
    dispatcher.on_agent_end(&ctx).await;

    let ctx = agent_core::context::SessionCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "test".to_string(),
        tools: vec![],
    };
    dispatcher.on_session_start(&ctx).await;
}