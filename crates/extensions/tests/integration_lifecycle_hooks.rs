use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use agent_core::context::{
    AgentEndCtx, BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx,
    SessionCtx, ToolExecutionEndCtx, ToolExecutionStartCtx, TurnEndCtx,
};
use agent_core::mutations::{
    BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation,
};
use agent_core::{SessionActor, SessionConfig};
use agent_core::HookDispatcher;
use agent_core::compaction::{CompactionActor, CompactionConfig};
use agent_core::file_ops::DefaultFileOperationExtractor;
use agent_core::test_utils::TestProvider;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;

fn make_compaction_actor(provider: Arc<dyn ai_provider::LlmProvider>) -> Arc<CompactionActor> {
    Arc::new(CompactionActor::new(
        CompactionConfig::default(),
        provider,
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    ))
}

// ============================================================================
// LifecycleRecorderExt — records every hook call
// ============================================================================

struct LifecycleRecorderExt {
    session_start_count: AtomicUsize,
    before_agent_start_count: AtomicUsize,
    before_provider_request_count: AtomicUsize,
    after_provider_response_count: AtomicUsize,
    turn_end_count: AtomicUsize,
    agent_end_count: AtomicUsize,
}

#[async_trait]
impl Extension for LifecycleRecorderExt {
    fn name(&self) -> &str { "lifecycle_recorder" }

    async fn on_session_start(&self, _ctx: &SessionCtx) {
        self.session_start_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_before_agent_start(&self, _ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
        self.before_agent_start_count.fetch_add(1, Ordering::SeqCst);
        BeforeAgentStartMutation::default()
    }

    async fn on_before_provider_request(&self, _ctx: &ProviderRequestCtx) -> ProviderRequestMutation {
        self.before_provider_request_count.fetch_add(1, Ordering::SeqCst);
        ProviderRequestMutation::default()
    }

    async fn on_after_provider_response(&self, _ctx: &ProviderResponseCtx) -> ProviderResponseMutation {
        self.after_provider_response_count.fetch_add(1, Ordering::SeqCst);
        ProviderResponseMutation::default()
    }

    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {
        self.turn_end_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {
        self.agent_end_count.fetch_add(1, Ordering::SeqCst);
    }
}

// ============================================================================
// ToolExecutionRecorderExt — records tool execution hooks
// ============================================================================

struct ToolExecutionRecorderExt {
    tool_execution_start_count: AtomicUsize,
    tool_execution_end_count: AtomicUsize,
}

#[async_trait]
impl Extension for ToolExecutionRecorderExt {
    fn name(&self) -> &str { "tool_execution_recorder" }

    async fn on_tool_execution_start(&self, _ctx: &ToolExecutionStartCtx) {
        self.tool_execution_start_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_tool_execution_end(&self, _ctx: &ToolExecutionEndCtx) {
        self.tool_execution_end_count.fetch_add(1, Ordering::SeqCst);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
async fn test_complete_lifecycle_hooks() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(LifecycleRecorderExt {
        session_start_count: AtomicUsize::new(0),
        before_agent_start_count: AtomicUsize::new(0),
        before_provider_request_count: AtomicUsize::new(0),
        after_provider_response_count: AtomicUsize::new(0),
        turn_end_count: AtomicUsize::new(0),
        agent_end_count: AtomicUsize::new(0),
    });

    let (handle, _) = ExtensionActor::spawn(ext.clone(), bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);

    let provider = TestProvider::text("response");
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".into(),
        model: "test".to_string(),
        provider: provider,
        hook_dispatcher: Arc::new(router),
        compaction_actor: compaction_actor,
        tools: vec![],
        store: None,
        skills: vec![],
    });

    // Wait for session_start observational hook (fire-and-forget)
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(ext.session_start_count.load(Ordering::SeqCst), 1, "session_start should fire once");

    // Run a prompt — this triggers before_agent_start, before_provider_request,
    // after_provider_response, and (observationally) turn_end + agent_end
    session.prompt("hello".to_string()).await.unwrap();

    // Wait for observational hooks to propagate through EventBus
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(ext.before_agent_start_count.load(Ordering::SeqCst), 1, "before_agent_start should fire once");
    assert_eq!(ext.before_provider_request_count.load(Ordering::SeqCst), 1, "before_provider_request should fire once");
    assert_eq!(ext.after_provider_response_count.load(Ordering::SeqCst), 1, "after_provider_response should fire once");
    assert_eq!(ext.turn_end_count.load(Ordering::SeqCst), 1, "turn_end should fire once");
    assert_eq!(ext.agent_end_count.load(Ordering::SeqCst), 1, "agent_end should fire once");
}

#[tokio::test]
async fn test_tool_execution_hooks_via_eventbus() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(ToolExecutionRecorderExt {
        tool_execution_start_count: AtomicUsize::new(0),
        tool_execution_end_count: AtomicUsize::new(0),
    });

    let (handle, _) = ExtensionActor::spawn(ext.clone(), bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);

    // Wait for ExtensionActor to subscribe to EventBus
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Emit tool execution events directly via router
    let start_ctx = ToolExecutionStartCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };
    let end_ctx = ToolExecutionEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        success: true,
    };

    router.on_tool_execution_start(&start_ctx).await;
    router.on_tool_execution_end(&end_ctx).await;

    // Wait for observational hooks to propagate through EventBus
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(ext.tool_execution_start_count.load(Ordering::SeqCst), 1, "tool_execution_start should fire once");
    assert_eq!(ext.tool_execution_end_count.load(Ordering::SeqCst), 1, "tool_execution_end should fire once");
}
