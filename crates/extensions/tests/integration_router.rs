use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use tokio::time::Duration;

use agent_core::context::{AgentEndCtx, ContextCtx, SessionCtx, ToolCallCtx, ToolResultCtx, TurnEndCtx};
use agent_core::mutations::{ContextMutation, HookDecision, ToolCallMutation, ToolResultMutation};
use agent_core::HookDispatcher;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;

// ============================================================================
// Helper: Mock extensions for testing HookRouter dispatch strategies
// ============================================================================

struct BlockExt {
    target_tool: String,
}

#[async_trait]
impl Extension for BlockExt {
    fn name(&self) -> &str { "blocker" }

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        if ctx.tool_name == self.target_tool {
            (HookDecision::Block { reason: format!("blocked {}", ctx.tool_name) }, ToolCallMutation::default())
        } else {
            (HookDecision::Continue, ToolCallMutation::default())
        }
    }
}

struct MutateResultExt {
    new_content: String,
}

#[async_trait]
impl Extension for MutateResultExt {
    fn name(&self) -> &str { "result_mutator" }

    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        ToolResultMutation {
            content: Some(vec![llm_client::Content::Text {
                text: self.new_content.clone(),
                text_signature: None,
            }]),
            details: Some(serde_json::json!({"mutated_by": self.new_content})),
            is_error: Some(false),
            terminate: None,
        }
    }
}

struct MutateContextExt;

#[async_trait]
impl Extension for MutateContextExt {
    fn name(&self) -> &str { "context_mutator" }

    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
        ContextMutation {
            messages: Some(vec![agent_core::AgentMessage::User(llm_client::UserMessage {
                content: vec![llm_client::Content::Text {
                    text: "mutated_context".to_string(),
                    text_signature: None,
                }],
                timestamp: std::time::SystemTime::now(),
            })]),
        }
    }
}

struct ObsCounterExt {
    turn_end_count: AtomicUsize,
    agent_end_count: AtomicUsize,
    session_start_count: AtomicUsize,
}

#[async_trait]
impl Extension for ObsCounterExt {
    fn name(&self) -> &str { "obs_counter" }

    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {
        self.turn_end_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {
        self.agent_end_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_session_start(&self, _ctx: &SessionCtx) {
        self.session_start_count.fetch_add(1, Ordering::SeqCst);
    }
}

struct TimeoutExt;

#[async_trait]
impl Extension for TimeoutExt {
    fn name(&self) -> &str { "slowpoke" }

    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        tokio::time::sleep(Duration::from_secs(10)).await;
        (HookDecision::Block { reason: "too late".to_string() }, ToolCallMutation::default())
    }
}

struct PanicExt;

#[async_trait]
impl Extension for PanicExt {
    fn name(&self) -> &str { "panicker" }

    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        panic!("intentional panic in on_tool_call")
    }

    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        panic!("intentional panic in on_tool_result")
    }

    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
        panic!("intentional panic in on_context")
    }
}

// ============================================================================
// Tests: First-block-wins
// ============================================================================

#[tokio::test]
async fn test_router_first_block_wins_ordering() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    // ext1: Continue, ext2: Block "dangerous_tool", ext3: Continue
    let ext1 = Arc::new(BlockExt { target_tool: "other".to_string() });
    let ext2 = Arc::new(BlockExt { target_tool: "dangerous_tool".to_string() });
    let ext3 = Arc::new(BlockExt { target_tool: "another".to_string() });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);
    let (h3, _) = ExtensionActor::spawn(ext3, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2, h3], bus);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "dangerous_tool".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, _mutation) = router.on_tool_call(&ctx).await;
    match decision {
        HookDecision::Block { reason } => {
            assert!(reason.contains("dangerous_tool"));
        }
        other => panic!("expected Block, got {:?}", other),
    }
}

#[tokio::test]
async fn test_router_all_continue_allows_tool() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(BlockExt { target_tool: "other".to_string() });
    let ext2 = Arc::new(BlockExt { target_tool: "different".to_string() });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "safe_tool".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, _mutation) = router.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));
}

// ============================================================================
// Tests: Chain merge for on_tool_result
// ============================================================================

#[tokio::test]
async fn test_router_chain_merge_tool_result() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(MutateResultExt { new_content: "first".to_string() });
    let ext2 = Arc::new(MutateResultExt { new_content: "second".to_string() });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ToolResultCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
        content: vec![llm_client::Content::Text { text: "original".to_string(), text_signature: None }],
        details: None,
        is_error: false,
    };

    let mutation = router.on_tool_result(&ctx).await;

    // Second handler's content wins
    assert!(mutation.content.is_some());
    match &mutation.content.as_ref().unwrap()[0] {
        llm_client::Content::Text { text, .. } => assert_eq!(text, "second"),
        _ => panic!("expected text content"),
    }
    assert!(mutation.details.is_some());
}

// ============================================================================
// Tests: Chain merge for on_context
// ============================================================================

#[tokio::test]
async fn test_router_chain_merge_context() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(MutateContextExt);
    let ext2 = Arc::new(MutateContextExt);

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ContextCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![agent_core::AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text { text: "original".to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        })],
    };

    let mutation = router.on_context(&ctx).await;

    // Should return mutated messages (not default)
    assert!(mutation.messages.is_some());
    let msgs = mutation.messages.unwrap();
    assert_eq!(msgs.len(), 1);
}

#[tokio::test]
async fn test_router_context_no_change_returns_default() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    // No extensions that mutate context
    let ext1 = Arc::new(BlockExt { target_tool: "other".to_string() });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let router = HookRouter::new(vec![h1], bus);

    let ctx = ContextCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![agent_core::AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text { text: "original".to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        })],
    };

    let mutation = router.on_context(&ctx).await;
    assert!(mutation.messages.is_none());
}

// ============================================================================
// Tests: Observational hooks via EventBus
// ============================================================================

#[tokio::test]
async fn test_router_observational_hooks_fire() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let counter = Arc::new(ObsCounterExt {
        turn_end_count: AtomicUsize::new(0),
        agent_end_count: AtomicUsize::new(0),
        session_start_count: AtomicUsize::new(0),
    });

    let (handle, _) = ExtensionActor::spawn(counter.clone(), bus.clone(), 8);

    // Give actor time to subscribe to EventBus
    tokio::time::sleep(Duration::from_millis(10)).await;

    let router = HookRouter::new(vec![handle], bus.clone());

    let turn_ctx = TurnEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        turn_index: 0,
        messages: vec![],
    };
    router.on_turn_end(&turn_ctx).await;

    let agent_ctx = AgentEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![],
    };
    router.on_agent_end(&agent_ctx).await;

    let session_ctx = SessionCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "test".to_string(),
        tools: vec![],
    };
    router.on_session_start(&session_ctx).await;

    // Give EventBus handlers time to process
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(counter.turn_end_count.load(Ordering::SeqCst), 1);
    assert_eq!(counter.agent_end_count.load(Ordering::SeqCst), 1);
    assert_eq!(counter.session_start_count.load(Ordering::SeqCst), 1);
}

// ============================================================================
// Tests: Timeout behavior
// ============================================================================

#[tokio::test]
async fn test_router_timeout_returns_default() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext = Arc::new(TimeoutExt);
    let (handle, _) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    // Should not hang; should return Continue due to timeout
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        router.on_tool_call(&ctx),
    ).await;

    assert!(result.is_ok());
    let (decision, _mutation) = result.unwrap();
    assert!(matches!(decision, HookDecision::Continue));
}

// ============================================================================
// Tests: Panic isolation across extension actors
// ============================================================================

#[tokio::test]
async fn test_router_panic_isolation_tool_call() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let panic_ext = Arc::new(PanicExt);
    let normal_ext = Arc::new(BlockExt { target_tool: "dangerous".to_string() });

    let (h1, _) = ExtensionActor::spawn(panic_ext, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(normal_ext, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "dangerous".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, _mutation) = router.on_tool_call(&ctx).await;
    // Panic ext returns Continue (default), then normal ext blocks
    assert!(matches!(decision, HookDecision::Block { .. }));
}

#[tokio::test]
async fn test_router_panic_isolation_tool_result() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let panic_ext = Arc::new(PanicExt);
    let mutate_ext = Arc::new(MutateResultExt { new_content: "recovered".to_string() });

    let (h1, _) = ExtensionActor::spawn(panic_ext, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(mutate_ext, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

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

    let mutation = router.on_tool_result(&ctx).await;
    // Panic ext returns default, then mutate ext applies
    assert!(mutation.content.is_some());
    match &mutation.content.as_ref().unwrap()[0] {
        llm_client::Content::Text { text, .. } => assert_eq!(text, "recovered"),
        _ => panic!("expected text content"),
    }
}

#[tokio::test]
async fn test_router_panic_isolation_context() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let panic_ext = Arc::new(PanicExt);
    let mutate_ext = Arc::new(MutateContextExt);

    let (h1, _) = ExtensionActor::spawn(panic_ext, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(mutate_ext, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ContextCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![],
    };

    let mutation = router.on_context(&ctx).await;
    // Panic ext returns default, then mutate ext applies
    assert!(mutation.messages.is_some());
}
