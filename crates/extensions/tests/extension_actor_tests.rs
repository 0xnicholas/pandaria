use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use agent_core::context::{ContextCtx, ToolCallCtx, ToolResultCtx};
use agent_core::mutations::{ContextMutation, HookDecision, ToolCallMutation, ToolResultMutation};
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};

// ============================================================================
// Helper extensions
// ============================================================================

struct BlockingExt {
    should_block: bool,
}

#[async_trait]
impl Extension for BlockingExt {
    fn name(&self) -> &str { "blocking" }

    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        if self.should_block {
            (HookDecision::Block { reason: "blocked".to_string() }, ToolCallMutation::default())
        } else {
            (HookDecision::Continue, ToolCallMutation::default())
        }
    }
}

struct ChainExt {
    mutation: ToolResultMutation,
}

#[async_trait]
impl Extension for ChainExt {
    fn name(&self) -> &str { "chain" }

    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        self.mutation.clone()
    }
}

struct ContextMutateExt {
    append_text: String,
}

#[async_trait]
impl Extension for ContextMutateExt {
    fn name(&self) -> &str { "context_mutator" }

    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        let mut messages = ctx.messages.clone();
        messages.push(agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: self.append_text.clone(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }));
        ContextMutation { messages: Some(messages) }
    }
}

struct SlowExt;

#[async_trait]
impl Extension for SlowExt {
    fn name(&self) -> &str { "slow" }

    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        tokio::time::sleep(Duration::from_secs(10)).await;
        ToolResultMutation::default()
    }
}

struct PanicToolCallExt;

#[async_trait]
impl Extension for PanicToolCallExt {
    fn name(&self) -> &str { "panic_tool_call" }

    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        panic!("intentional panic in on_tool_call")
    }
}

struct PanicToolResultExt;

#[async_trait]
impl Extension for PanicToolResultExt {
    fn name(&self) -> &str { "panic_tool_result" }

    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        panic!("intentional panic in on_tool_result")
    }
}

struct PanicContextExt;

#[async_trait]
impl Extension for PanicContextExt {
    fn name(&self) -> &str { "panic_context" }

    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
        panic!("intentional panic in on_context")
    }
}

// ============================================================================
// Tests: Spawn and basic operation
// ============================================================================

#[tokio::test]
async fn test_spawn_and_basic_operation() {
    let ext = Arc::new(BlockingExt { should_block: false });
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, join_handle) = ExtensionActor::spawn(ext, bus, 8);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, _mutation) = handle.on_tool_call(ctx).await;
    assert!(matches!(decision, HookDecision::Continue));

    // Clean up
    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(1), join_handle).await;
}

// ============================================================================
// Tests: Blocking hook response
// ============================================================================

#[tokio::test]
async fn test_blocking_hook_response() {
    let ext = Arc::new(BlockingExt { should_block: true });
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, _mutation) = handle.on_tool_call(ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
}

// ============================================================================
// Tests: Chain hook response
// ============================================================================

#[tokio::test]
async fn test_chain_hook_response_tool_result() {
    let ext = Arc::new(ChainExt {
        mutation: ToolResultMutation {
            content: Some(vec![ai_provider::Content::Text {
                text: "mutated".to_string(),
                text_signature: None,
            }]),
            details: Some(serde_json::json!({"key": "value"})),
            is_error: Some(false),
            terminate: None,
        },
    });
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

    let ctx = ToolResultCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
        content: vec![],
        details: None,
        is_error: false,
    };

    let mutation = handle.on_tool_result(ctx).await;
    assert!(mutation.content.is_some());
    assert!(mutation.details.is_some());
    assert_eq!(mutation.is_error, Some(false));
}

#[tokio::test]
async fn test_chain_hook_response_context() {
    let ext = Arc::new(ContextMutateExt { append_text: "appended".to_string() });
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

    let ctx = ContextCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![],
    };

    let mutation = handle.on_context(ctx).await;
    assert!(mutation.messages.is_some());
    assert_eq!(mutation.messages.unwrap().len(), 1);
}

// ============================================================================
// Tests: Timeout returns default
// ============================================================================

#[tokio::test]
async fn test_timeout_returns_default() {
    let ext = Arc::new(SlowExt);
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

    let ctx = ToolResultCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
        content: vec![],
        details: None,
        is_error: false,
    };

    // Should timeout and return default (not hang)
    let result = tokio::time::timeout(Duration::from_secs(2), handle.on_tool_result(ctx)).await;
    assert!(result.is_ok(), "should not hang");
    let mutation = result.unwrap();
    assert!(mutation.content.is_none(), "should return default after timeout");
}

// ============================================================================
// Tests: Panic isolation
// ============================================================================

#[tokio::test]
async fn test_panic_isolation_tool_call() {
    let ext = Arc::new(PanicToolCallExt);
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    // Panic should be caught; actor should survive and return Continue
    let (decision, _mutation) = handle.on_tool_call(ctx).await;
    assert!(matches!(decision, HookDecision::Continue));

    // Actor should still be alive
    let ctx2 = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c2".to_string(),
        input: serde_json::json!({}),
    };
    let (decision2, _mutation2) = handle.on_tool_call(ctx2).await;
    assert!(matches!(decision2, HookDecision::Continue));
}

#[tokio::test]
async fn test_panic_isolation_tool_result() {
    let ext = Arc::new(PanicToolResultExt);
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

    let ctx = ToolResultCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
        content: vec![],
        details: None,
        is_error: false,
    };

    let mutation = handle.on_tool_result(ctx).await;
    assert!(mutation.content.is_none());
    assert!(mutation.details.is_none());
    assert!(mutation.is_error.is_none());
    assert!(mutation.terminate.is_none());
}

#[tokio::test]
async fn test_panic_isolation_context() {
    let ext = Arc::new(PanicContextExt);
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

    let ctx = ContextCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![],
    };

    let mutation = handle.on_context(ctx).await;
    assert!(mutation.messages.is_none());
}

// ============================================================================
// Tests: Actor shutdown on handle drop
// ============================================================================

#[tokio::test]
async fn test_actor_shutdown_on_handle_drop() {
    let ext = Arc::new(BlockingExt { should_block: false });
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, join_handle) = ExtensionActor::spawn(ext, bus, 8);

    // Drop the handle — this closes the mpsc channel
    drop(handle);

    // Actor should exit its loop and the JoinHandle should resolve
    let result = tokio::time::timeout(Duration::from_secs(2), join_handle).await;
    assert!(result.is_ok(), "actor should exit after handle is dropped");
    assert!(result.unwrap().is_ok(), "actor task should complete successfully");
}
