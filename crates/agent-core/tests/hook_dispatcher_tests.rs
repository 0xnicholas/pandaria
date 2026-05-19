use agent_core::context::ToolCallCtx;
use agent_core::hook::dispatcher::HookDispatcher;
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
    let mut ctx = ToolCallCtx::new("t1", "s1", "test_tool", "call_1");
    ctx.input = serde_json::json!({});

    let (decision, _mutation) = dispatcher.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));
}

#[tokio::test]
async fn test_custom_dispatcher_blocks() {
    let dispatcher = BlockingDispatcher;
    let mut ctx = ToolCallCtx::new("t1", "s1", "test_tool", "call_1");
    ctx.input = serde_json::json!({});

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
    let ctx = agent_core::context::ToolResultCtx::new("t1", "s1", "test", "call_1");

    let mutation = dispatcher.on_tool_result(&ctx).await;
    assert!(mutation.content.is_none());
    assert!(mutation.details.is_none());
    assert!(mutation.is_error.is_none());

    let ctx = agent_core::context::ContextCtx::new("t1", "s1");

    let mutation = dispatcher.on_context(&ctx).await;
    assert!(mutation.messages.is_none());

    let ctx = agent_core::context::TurnEndCtx::new("t1", "s1", 0, ai_provider::Usage {
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });

    // Observational hooks should not panic
    dispatcher.on_turn_end(&ctx).await;

    let ctx = agent_core::context::AgentEndCtx::new("t1", "s1");
    dispatcher.on_agent_end(&ctx).await;

    let mut ctx = agent_core::context::SessionCtx::new("t1", "s1");
    ctx.system_prompt = "test".to_string();
    dispatcher.on_session_start(&ctx).await;
}
