use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use agent_core::context::{AgentEndCtx, ContextCtx, SessionCtx, ToolCallCtx, ToolResultCtx, TurnEndCtx};
use agent_core::mutations::{ContextMutation, HookDecision, ToolCallMutation, ToolResultMutation};
use agent_core::HookDispatcher;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;

// ============================================================================
// Helper extensions
// ============================================================================

struct ContinueExt {
    name: String,
}

#[async_trait]
impl Extension for ContinueExt {
    fn name(&self) -> &str { &self.name }
}

struct BlockExt {
    name: String,
    target_tool: String,
}

#[async_trait]
impl Extension for BlockExt {
    fn name(&self) -> &str { &self.name }

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        if ctx.tool_name == self.target_tool {
            (HookDecision::Block { reason: format!("blocked by {}", self.name) }, ToolCallMutation::default())
        } else {
            (HookDecision::Continue, ToolCallMutation::default())
        }
    }
}

struct MutateResultExt {
    name: String,
    content: Option<String>,
    details: Option<serde_json::Value>,
    is_error: Option<bool>,
    terminate: Option<bool>,
}

#[async_trait]
impl Extension for MutateResultExt {
    fn name(&self) -> &str { &self.name }

    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        ToolResultMutation {
            content: self.content.as_ref().map(|s| {
                vec![ai_provider::Content::Text { text: s.clone(), text_signature: None }]
            }),
            details: self.details.clone(),
            is_error: self.is_error,
            terminate: self.terminate,
        }
    }
}

struct MutateContextExt {
    name: String,
    append_message: Option<String>,
}

#[async_trait]
impl Extension for MutateContextExt {
    fn name(&self) -> &str { &self.name }

    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        if let Some(ref text) = self.append_message {
            let mut messages = ctx.messages.clone();
            messages.push(agent_core::AgentMessage::User(ai_provider::UserMessage {
                content: vec![ai_provider::Content::Text {
                    text: text.clone(),
                    text_signature: None,
                }],
                timestamp: std::time::SystemTime::now(),
            }));
            ContextMutation { messages: Some(messages) }
        } else {
            ContextMutation::default()
        }
    }
}

struct ObsCounterExt {
    name: String,
    turn_end_count: AtomicUsize,
    agent_end_count: AtomicUsize,
    session_start_count: AtomicUsize,
}

#[async_trait]
impl Extension for ObsCounterExt {
    fn name(&self) -> &str { &self.name }

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

struct TimeoutExt {
    name: String,
}

#[async_trait]
impl Extension for TimeoutExt {
    fn name(&self) -> &str { &self.name }

    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        tokio::time::sleep(Duration::from_secs(10)).await;
        (HookDecision::Block { reason: "too late".to_string() }, ToolCallMutation::default())
    }
}

struct PanicExt {
    name: String,
    panic_on_tool_call: bool,
    panic_on_tool_result: bool,
    panic_on_context: bool,
}

#[async_trait]
impl Extension for PanicExt {
    fn name(&self) -> &str { &self.name }

    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        if self.panic_on_tool_call {
            panic!("intentional panic in on_tool_call")
        }
        (HookDecision::Continue, ToolCallMutation::default())
    }

    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        if self.panic_on_tool_result {
            panic!("intentional panic in on_tool_result")
        }
        ToolResultMutation::default()
    }

    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
        if self.panic_on_context {
            panic!("intentional panic in on_context")
        }
        ContextMutation::default()
    }
}

// ============================================================================
// Tests: Empty extension list
// ============================================================================

#[tokio::test]
async fn test_empty_extension_list() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let router = HookRouter::new(vec![], bus.clone());

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, _mutation) = router.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));

    let result_ctx = ToolResultCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
        content: vec![],
        details: None,
        is_error: false,
    };
    let mutation = router.on_tool_result(&result_ctx).await;
    assert!(mutation.content.is_none());
    assert!(mutation.details.is_none());
    assert!(mutation.is_error.is_none());
    assert!(mutation.terminate.is_none());

    let context_ctx = ContextCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![],
    };
    let mutation = router.on_context(&context_ctx).await;
    assert!(mutation.messages.is_none());
}

// ============================================================================
// Tests: First-block-wins
// ============================================================================

#[tokio::test]
async fn test_first_block_wins_basic() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(BlockExt { name: "blocker".to_string(), target_tool: "dangerous".to_string() });
    let ext2 = Arc::new(ContinueExt { name: "passer".to_string() });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "dangerous".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, _mutation) = router.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
}

#[tokio::test]
async fn test_first_block_wins_ordering() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    // ext1: Continue, ext2: Block, ext3: Continue
    let ext1 = Arc::new(ContinueExt { name: "first".to_string() });
    let ext2 = Arc::new(BlockExt { name: "second".to_string(), target_tool: "dangerous".to_string() });
    let ext3 = Arc::new(ContinueExt { name: "third".to_string() });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);
    let (h3, _) = ExtensionActor::spawn(ext3, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2, h3], bus);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "dangerous".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, _mutation) = router.on_tool_call(&ctx).await;
    match decision {
        HookDecision::Block { reason } => {
            assert!(reason.contains("second"));
        }
        other => panic!("expected Block, got {:?}", other),
    }
}

#[tokio::test]
async fn test_all_continue_allows_tool() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(ContinueExt { name: "first".to_string() });
    let ext2 = Arc::new(ContinueExt { name: "second".to_string() });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "safe".to_string(),
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
async fn test_chain_merge_tool_result_overwrite() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(MutateResultExt {
        name: "first".to_string(),
        content: Some("first_content".to_string()),
        details: None,
        is_error: None,
        terminate: None,
    });
    let ext2 = Arc::new(MutateResultExt {
        name: "second".to_string(),
        content: Some("second_content".to_string()),
        details: None,
        is_error: None,
        terminate: None,
    });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ToolResultCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
        content: vec![ai_provider::Content::Text { text: "original".to_string(), text_signature: None }],
        details: None,
        is_error: false,
    };

    let mutation = router.on_tool_result(&ctx).await;

    // Second handler's content wins
    assert!(mutation.content.is_some());
    match &mutation.content.as_ref().unwrap()[0] {
        ai_provider::Content::Text { text, .. } => assert_eq!(text, "second_content"),
        _ => panic!("expected text content"),
    }
}

#[tokio::test]
async fn test_chain_merge_tool_result_partial() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    // ext1: only sets is_error, ext2: only sets content
    let ext1 = Arc::new(MutateResultExt {
        name: "error_setter".to_string(),
        content: None,
        details: None,
        is_error: Some(true),
        terminate: None,
    });
    let ext2 = Arc::new(MutateResultExt {
        name: "content_setter".to_string(),
        content: Some("data".to_string()),
        details: None,
        is_error: None,
        terminate: None,
    });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

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

    // Both mutations should be present
    assert!(mutation.content.is_some(), "content should be set by second handler");
    assert_eq!(mutation.is_error, Some(true), "is_error should be set by first handler");
}

#[tokio::test]
async fn test_chain_merge_tool_result_terminate() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    // ext1: sets terminate=true, ext2: sets content
    let ext1 = Arc::new(MutateResultExt {
        name: "terminator".to_string(),
        content: None,
        details: None,
        is_error: None,
        terminate: Some(true),
    });
    let ext2 = Arc::new(MutateResultExt {
        name: "content_setter".to_string(),
        content: Some("data".to_string()),
        details: None,
        is_error: None,
        terminate: None,
    });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

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

    // Terminate should propagate
    assert_eq!(mutation.terminate, Some(true), "terminate should be propagated");
    assert!(mutation.content.is_some(), "content should also be set");
}

// ============================================================================
// Tests: Chain merge for on_context
// ============================================================================

#[tokio::test]
async fn test_chain_merge_context_mutation() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(MutateContextExt { name: "appender1".to_string(), append_message: Some("msg1".to_string()) });
    let ext2 = Arc::new(MutateContextExt { name: "appender2".to_string(), append_message: Some("msg2".to_string()) });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ContextCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text { text: "original".to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        })],
    };

    let mutation = router.on_context(&ctx).await;

    // Should have: original + msg1 + msg2
    assert!(mutation.messages.is_some());
    let msgs = mutation.messages.unwrap();
    assert_eq!(msgs.len(), 3);
}

#[tokio::test]
async fn test_context_no_change_returns_default() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(ContinueExt { name: "noop".to_string() });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let router = HookRouter::new(vec![h1], bus);

    let ctx = ContextCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text { text: "original".to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        })],
    };

    let mutation = router.on_context(&ctx).await;
    assert!(mutation.messages.is_none(), "should return default when no handler modified messages");
}

// ============================================================================
// Tests: Observational hooks via EventBus
// ============================================================================

#[tokio::test]
async fn test_observational_hooks_fire() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let counter = Arc::new(ObsCounterExt {
        name: "counter".to_string(),
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
async fn test_timeout_returns_continue() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext = Arc::new(TimeoutExt { name: "slowpoke".to_string() });
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
async fn test_panic_isolation_tool_call() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let panic_ext = Arc::new(PanicExt {
        name: "panicker".to_string(),
        panic_on_tool_call: true,
        panic_on_tool_result: false,
        panic_on_context: false,
    });
    let normal_ext = Arc::new(BlockExt { name: "blocker".to_string(), target_tool: "dangerous".to_string() });

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
async fn test_panic_isolation_tool_result() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let panic_ext = Arc::new(PanicExt {
        name: "panicker".to_string(),
        panic_on_tool_call: false,
        panic_on_tool_result: true,
        panic_on_context: false,
    });
    let mutate_ext = Arc::new(MutateResultExt {
        name: "mutator".to_string(),
        content: Some("recovered".to_string()),
        details: None,
        is_error: None,
        terminate: None,
    });

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
        ai_provider::Content::Text { text, .. } => assert_eq!(text, "recovered"),
        _ => panic!("expected text content"),
    }
}

#[tokio::test]
async fn test_panic_isolation_context() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let panic_ext = Arc::new(PanicExt {
        name: "panicker".to_string(),
        panic_on_tool_call: false,
        panic_on_tool_result: false,
        panic_on_context: true,
    });
    let mutate_ext = Arc::new(MutateContextExt { name: "mutator".to_string(), append_message: Some("recovered".to_string()) });

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
    assert_eq!(mutation.messages.unwrap().len(), 1);
}
