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
// Multi-extension scenario: Audit + RateLimit + ToolGuard
// ============================================================================

struct AuditExt {
    tool_calls: AtomicUsize,
    tool_results: AtomicUsize,
    context_mutations: AtomicUsize,
}

#[async_trait]
impl Extension for AuditExt {
    fn name(&self) -> &str { "audit" }

    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        self.tool_calls.fetch_add(1, Ordering::SeqCst);
        (HookDecision::Continue, ToolCallMutation::default())
    }

    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        self.tool_results.fetch_add(1, Ordering::SeqCst);
        ToolResultMutation::default()
    }

    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
        self.context_mutations.fetch_add(1, Ordering::SeqCst);
        ContextMutation::default()
    }
}

struct RateLimitExt {
    allowed_tools: Vec<String>,
    blocked_count: AtomicUsize,
}

#[async_trait]
impl Extension for RateLimitExt {
    fn name(&self) -> &str { "rate_limit" }

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        if self.allowed_tools.contains(&ctx.tool_name) {
            (HookDecision::Continue, ToolCallMutation::default())
        } else {
            self.blocked_count.fetch_add(1, Ordering::SeqCst);
            (HookDecision::Block { reason: "rate limited".to_string() }, ToolCallMutation::default())
        }
    }
}

struct ToolGuardExt {
    forbidden_tools: Vec<String>,
    blocked_count: AtomicUsize,
}

#[async_trait]
impl Extension for ToolGuardExt {
    fn name(&self) -> &str { "tool_guard" }

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        if self.forbidden_tools.contains(&ctx.tool_name) {
            self.blocked_count.fetch_add(1, Ordering::SeqCst);
            (HookDecision::Block { reason: format!("forbidden: {}", ctx.tool_name) }, ToolCallMutation::default())
        } else {
            (HookDecision::Continue, ToolCallMutation::default())
        }
    }
}

struct ContextAppendExt {
    prefix: String,
}

#[async_trait]
impl Extension for ContextAppendExt {
    fn name(&self) -> &str { "context_appender" }

    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        let mut messages = ctx.messages.clone();
        messages.push(agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: self.prefix.clone(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }));
        ContextMutation { messages: Some(messages) }
    }
}

struct ResultMutatorExt {
    append_text: String,
}

#[async_trait]
impl Extension for ResultMutatorExt {
    fn name(&self) -> &str { "result_mutator" }

    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
        let mut content = ctx.content.clone();
        content.push(ai_provider::Content::Text {
            text: self.append_text.clone(),
            text_signature: None,
        });
        ToolResultMutation {
            content: Some(content),
            details: None,
            is_error: None,
            terminate: None,
        }
    }
}

struct ObsRecorderExt {
    session_starts: AtomicUsize,
    turn_ends: AtomicUsize,
    agent_ends: AtomicUsize,
}

#[async_trait]
impl Extension for ObsRecorderExt {
    fn name(&self) -> &str { "obs_recorder" }

    async fn on_session_start(&self, _ctx: &SessionCtx) {
        self.session_starts.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {
        self.turn_ends.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {
        self.agent_ends.fetch_add(1, Ordering::SeqCst);
    }
}

struct PanicToolCallExt;

#[async_trait]
impl Extension for PanicToolCallExt {
    fn name(&self) -> &str { "panic_tool_call" }

    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        panic!("intentional panic in tool_call")
    }
}

struct PanicToolResultExt;

#[async_trait]
impl Extension for PanicToolResultExt {
    fn name(&self) -> &str { "panic_tool_result" }

    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        panic!("intentional panic in tool_result")
    }
}

struct PanicContextExt;

#[async_trait]
impl Extension for PanicContextExt {
    fn name(&self) -> &str { "panic_context" }

    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
        panic!("intentional panic in context")
    }
}

// ============================================================================
// Tests: Multi-extension dispatch
// ============================================================================

#[tokio::test]
async fn test_multi_extension_first_block_wins_order() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    // Order: rate_limit (allows "fast_tool" and "dangerous_tool") -> tool_guard (blocks "dangerous_tool")
    let rate_limit = Arc::new(RateLimitExt {
        allowed_tools: vec!["fast_tool".to_string(), "dangerous_tool".to_string()],
        blocked_count: AtomicUsize::new(0),
    });
    let tool_guard = Arc::new(ToolGuardExt {
        forbidden_tools: vec!["dangerous_tool".to_string()],
        blocked_count: AtomicUsize::new(0),
    });

    let (h1, _) = ExtensionActor::spawn(rate_limit.clone(), bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(tool_guard.clone(), bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    // Test 1: "slow_tool" - rate_limit blocks first (first-block-wins)
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "slow_tool".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };
    let (decision, _mutation) = router.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
    assert_eq!(rate_limit.blocked_count.load(Ordering::SeqCst), 1);
    assert_eq!(tool_guard.blocked_count.load(Ordering::SeqCst), 0);

    // Test 2: "dangerous_tool" - rate_limit passes, tool_guard blocks
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "dangerous_tool".to_string(),
        tool_call_id: "c2".to_string(),
        input: serde_json::json!({}),
    };
    let (decision, _mutation) = router.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
    assert_eq!(rate_limit.blocked_count.load(Ordering::SeqCst), 1);
    assert_eq!(tool_guard.blocked_count.load(Ordering::SeqCst), 1);

    // Test 3: "fast_tool" - both pass
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "fast_tool".to_string(),
        tool_call_id: "c3".to_string(),
        input: serde_json::json!({}),
    };
    let (decision, _mutation) = router.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));
}

#[tokio::test]
async fn test_multi_extension_chain_merge_context() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(ContextAppendExt { prefix: "prefix".to_string() });
    let ext2 = Arc::new(ContextAppendExt { prefix: "suffix".to_string() });

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
    let messages = mutation.messages.unwrap();

    // Should have: original + prefix + suffix
    assert_eq!(messages.len(), 3);
}

#[tokio::test]
async fn test_multi_extension_chain_merge_tool_result() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(ResultMutatorExt { append_text: "_A".to_string() });
    let ext2 = Arc::new(ResultMutatorExt { append_text: "_B".to_string() });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ToolResultCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
        content: vec![ai_provider::Content::Text { text: "base".to_string(), text_signature: None }],
        details: None,
        is_error: false,
    };

    let mutation = router.on_tool_result(&ctx).await;
    let content = mutation.content.unwrap();

    // Should have: base + _A + _B (each ext appends to previous)
    assert_eq!(content.len(), 3);
    match &content[2] {
        ai_provider::Content::Text { text, .. } => assert_eq!(text, "_B"),
        _ => panic!("expected text"),
    }
}

#[tokio::test]
async fn test_multi_extension_observational_hooks() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(ObsRecorderExt {
        session_starts: AtomicUsize::new(0),
        turn_ends: AtomicUsize::new(0),
        agent_ends: AtomicUsize::new(0),
    });
    let ext2 = Arc::new(ObsRecorderExt {
        session_starts: AtomicUsize::new(0),
        turn_ends: AtomicUsize::new(0),
        agent_ends: AtomicUsize::new(0),
    });

    let (h1, _) = ExtensionActor::spawn(ext1.clone(), bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2.clone(), bus.clone(), 8);

    // Give actors time to subscribe to EventBus
    tokio::time::sleep(Duration::from_millis(10)).await;

    let router = HookRouter::new(vec![h1, h2], bus);

    let session_ctx = SessionCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "test".to_string(),
        tools: vec![],
    };
    router.on_session_start(&session_ctx).await;

    let turn_ctx = TurnEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        turn_index: 0,
        messages: vec![],
        usage: ai_provider::Usage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
    };
    router.on_turn_end(&turn_ctx).await;

    let agent_ctx = AgentEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![],
    };
    router.on_agent_end(&agent_ctx).await;

    // Give EventBus handlers time
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Both extensions should have received all observational events
    assert_eq!(ext1.session_starts.load(Ordering::SeqCst), 1);
    assert_eq!(ext1.turn_ends.load(Ordering::SeqCst), 1);
    assert_eq!(ext1.agent_ends.load(Ordering::SeqCst), 1);

    assert_eq!(ext2.session_starts.load(Ordering::SeqCst), 1);
    assert_eq!(ext2.turn_ends.load(Ordering::SeqCst), 1);
    assert_eq!(ext2.agent_ends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_multi_extension_audit_trail() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let audit = Arc::new(AuditExt {
        tool_calls: AtomicUsize::new(0),
        tool_results: AtomicUsize::new(0),
        context_mutations: AtomicUsize::new(0),
    });

    let (h1, _) = ExtensionActor::spawn(audit.clone(), bus.clone(), 8);
    let router = HookRouter::new(vec![h1], bus);

    // Dispatch a tool call
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };
    let _ = router.on_tool_call(&ctx).await;

    // Dispatch a tool result
    let ctx = ToolResultCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
        content: vec![],
        details: None,
        is_error: false,
    };
    let _ = router.on_tool_result(&ctx).await;

    // Dispatch a context mutation
    let ctx = ContextCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![],
    };
    let _ = router.on_context(&ctx).await;

    assert_eq!(audit.tool_calls.load(Ordering::SeqCst), 1);
    assert_eq!(audit.tool_results.load(Ordering::SeqCst), 1);
    assert_eq!(audit.context_mutations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_multi_extension_panic_isolation_preserves_others() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let panic_ext = Arc::new(PanicToolCallExt);
    let normal_ext = Arc::new(ToolGuardExt {
        forbidden_tools: vec!["bad".to_string()],
        blocked_count: AtomicUsize::new(0),
    });
    let audit_ext = Arc::new(AuditExt {
        tool_calls: AtomicUsize::new(0),
        tool_results: AtomicUsize::new(0),
        context_mutations: AtomicUsize::new(0),
    });

    let (h1, _) = ExtensionActor::spawn(panic_ext, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(normal_ext.clone(), bus.clone(), 8);
    let (h3, _) = ExtensionActor::spawn(audit_ext.clone(), bus.clone(), 8);

    // Give actors time to fully initialize
    tokio::time::sleep(Duration::from_millis(10)).await;

    let router = HookRouter::new(vec![h1, h2, h3], bus);

    // Tool call: panic ext returns Continue (default), then normal ext blocks "bad"
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "bad".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };
    let (decision, _mutation) = router.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
    assert_eq!(normal_ext.blocked_count.load(Ordering::SeqCst), 1);
    // audit_ext is never called because normal_ext blocks first (first-block-wins)

    // Tool result: test with panic in result
    let bus2 = Arc::new(EventBus::<ObsEvent>::new(16));
    let panic_result = Arc::new(PanicToolResultExt);
    let audit_result = Arc::new(AuditExt {
        tool_calls: AtomicUsize::new(0),
        tool_results: AtomicUsize::new(0),
        context_mutations: AtomicUsize::new(0),
    });

    let (h1, _) = ExtensionActor::spawn(panic_result, bus2.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(audit_result.clone(), bus2.clone(), 8);

    let router2 = HookRouter::new(vec![h1, h2], bus2);

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
    let mutation = router2.on_tool_result(&ctx).await;
    // Panic ext returns default, audit ext also returns default
    assert!(mutation.content.is_none());
    assert_eq!(audit_result.tool_results.load(Ordering::SeqCst), 1);

    // Context: test with panic in context
    let bus3 = Arc::new(EventBus::<ObsEvent>::new(16));
    let panic_ctx = Arc::new(PanicContextExt);
    let mutate_ctx = Arc::new(ContextAppendExt { prefix: "recovered".to_string() });

    let (h1, _) = ExtensionActor::spawn(panic_ctx, bus3.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(mutate_ctx, bus3.clone(), 8);

    let router3 = HookRouter::new(vec![h1, h2], bus3);

    let ctx = ContextCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        messages: vec![],
    };
    let mutation = router3.on_context(&ctx).await;
    // Panic ext returns default, mutate ext appends
    assert!(mutation.messages.is_some());
    assert_eq!(mutation.messages.unwrap().len(), 1);
}
