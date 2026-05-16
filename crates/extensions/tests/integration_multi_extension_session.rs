use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use agent_core::context::{ToolCallCtx, TurnEndCtx, AgentEndCtx};
use agent_core::mutations::{HookDecision, ToolCallMutation};
use agent_core::SessionActor;
use agent_core::SessionEntry;
use agent_core::SessionStore;
use agent_core::error::AgentError;
use agent_core::compaction::{CompactionActor, CompactionConfig};
use agent_core::file_ops::DefaultFileOperationExtractor;
use agent_core::types::{AgentToolResult, AgentMessage};
use agent_core::test_utils::{TestProvider, TestResponse, TestToolCall};
use extensions::host::extension::Extension;
use extensions::host::manager::ExtensionManager;
use ai_provider::{Content, ToolDef};

// ============================================================================
// Mock Extensions
// ============================================================================

struct ToolGuardExt;

#[async_trait]
impl Extension for ToolGuardExt {
    fn name(&self) -> &str { "tool_guard" }

    async fn on_tool_call(
        &self, ctx: &ToolCallCtx
    ) -> (HookDecision, ToolCallMutation) {
        if ctx.tool_name == "dangerous_tool" {
            (HookDecision::Block { reason: "forbidden".to_string() }, ToolCallMutation::default())
        } else {
            (HookDecision::Continue, ToolCallMutation::default())
        }
    }
}

struct AuditExt {
    tool_calls: AtomicUsize,
}

#[async_trait]
impl Extension for AuditExt {
    fn name(&self) -> &str { "audit" }

    async fn on_tool_call(
        &self, _ctx: &ToolCallCtx
    ) -> (HookDecision, ToolCallMutation) {
        self.tool_calls.fetch_add(1, Ordering::SeqCst);
        (HookDecision::Continue, ToolCallMutation::default())
    }
}

struct ToolProviderExt;

#[async_trait]
impl Extension for ToolProviderExt {
    fn name(&self) -> &str { "tool_provider" }

    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "safe_tool".to_string(),
                description: "A safe tool".to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            },
            ToolDef {
                name: "dangerous_tool".to_string(),
                description: "A dangerous tool".to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            },
        ]
    }

    async fn execute_tool(
        &self, _tool_call_id: &str, _params: serde_json::Value
    ) -> Result<AgentToolResult, AgentError> {
        Ok(AgentToolResult {
            content: vec![Content::Text { text: "executed".to_string(), text_signature: None }],
            details: None, is_error: false, terminate: false,
        })
    }
}

struct LifecycleExt {
    turn_end_count: AtomicUsize,
    agent_end_count: AtomicUsize,
}

#[async_trait]
impl Extension for LifecycleExt {
    fn name(&self) -> &str { "lifecycle" }

    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {
        self.turn_end_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {
        self.agent_end_count.fetch_add(1, Ordering::SeqCst);
    }
}

// ============================================================================
// MemoryStore
// ============================================================================

struct MemoryStore {
    data: std::sync::Mutex<Vec<(String, String, Vec<SessionEntry>)>>,
}

impl MemoryStore {
    fn new() -> Self { Self { data: std::sync::Mutex::new(Vec::new()) } }
}

#[async_trait]
impl SessionStore for MemoryStore {
    async fn save_session(&self, tenant_id: &str, session_id: &str, entries: &[SessionEntry]
    ) -> Result<(), AgentError> {
        self.data.lock().unwrap().push((
            tenant_id.to_string(), session_id.to_string(), entries.to_vec()
        ));
        Ok(())
    }

    async fn load_session(&self, tenant_id: &str, session_id: &str
    ) -> Result<Vec<SessionEntry>, AgentError> {
        let data = self.data.lock().unwrap();
        Ok(data.iter().rev()
            .find_map(|(tid, sid, msgs)| if tid == tenant_id && sid == session_id { Some(msgs.clone()) } else { None })
            .unwrap_or_default())
    }

    async fn delete_session(&self, tenant_id: &str, session_id: &str
    ) -> Result<(), AgentError> {
        let mut data = self.data.lock().unwrap();
        data.retain(|(tid, sid, _)| !(tid == tenant_id && sid == session_id));
        Ok(())
    }

    async fn list_sessions(&self, tenant_id: &str
    ) -> Result<Vec<String>, AgentError> {
        let data = self.data.lock().unwrap();
        let mut sids: Vec<String> = data
            .iter()
            .filter(|(tid, _, _)| tid == tenant_id)
            .map(|(_, sid, _)| sid.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        sids.sort();
        Ok(sids)
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn make_compaction_actor(provider: Arc<dyn ai_provider::LlmProvider>) -> Arc<CompactionActor> {
    Arc::new(CompactionActor::new(
        CompactionConfig::default(),
        provider,
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    ))
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
async fn test_multi_extension_collaboration() {
    let _ = tracing_subscriber::fmt().try_init();

    let tool_guard = Arc::new(ToolGuardExt);
    let audit = Arc::new(AuditExt { tool_calls: AtomicUsize::new(0) });
    let tool_provider = Arc::new(ToolProviderExt);
    let lifecycle = Arc::new(LifecycleExt {
        turn_end_count: AtomicUsize::new(0),
        agent_end_count: AtomicUsize::new(0),
    });

    let manager = ExtensionManager::new(vec![
        tool_guard.clone(),
        audit.clone(),
        tool_provider.clone(),
        lifecycle.clone(),
    ]);
    let (hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);

    let provider = TestProvider::sequence(vec![
        TestResponse::ToolCalls(vec![TestToolCall::new(
            "call_1",
            "dangerous_tool",
            serde_json::json!({}),
        )]),
        TestResponse::ToolCalls(vec![TestToolCall::new(
            "call_2",
            "safe_tool",
            serde_json::json!({}),
        )]),
        TestResponse::Text("done".into()),
    ]);

    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".into(),
        "test".to_string(),
        provider,
        Arc::new(hook_router),
        compaction_actor,
        tools,
        None,
    vec![],
    );

    let results = session.prompt("call tools".to_string()).await.unwrap();

    // Should have multiple turns:
    // Turn 1: assistant(dangerous_tool) + tool_result(blocked)
    // Turn 2: assistant(safe_tool) + tool_result(executed)
    // Turn 3: assistant("done")
    assert!(results.len() >= 3, "expected at least 3 messages, got {}", results.len());

    // Find the dangerous_tool result — should be blocked (is_error=true)
    let dangerous_result = results.iter().find(|m| {
        if let AgentMessage::ToolResult(tr) = m {
            tr.tool_name == "dangerous_tool"
        } else {
            false
        }
    });
    assert!(dangerous_result.is_some(), "expected dangerous_tool result");
    if let AgentMessage::ToolResult(tr) = dangerous_result.unwrap() {
        assert!(tr.is_error, "dangerous_tool should be blocked (is_error=true)");
    }

    // Find the safe_tool result — should execute successfully (is_error=false)
    let safe_result = results.iter().find(|m| {
        if let AgentMessage::ToolResult(tr) = m {
            tr.tool_name == "safe_tool"
        } else {
            false
        }
    });
    assert!(safe_result.is_some(), "expected safe_tool result");
    if let AgentMessage::ToolResult(tr) = safe_result.unwrap() {
        assert!(!tr.is_error, "safe_tool should succeed (is_error=false)");
    }

    // AuditExt: first-block-wins means ToolGuard blocks before Audit counts.
    // Audit should see safe_tool but not dangerous_tool.
    assert!(
        audit.tool_calls.load(Ordering::SeqCst) >= 1,
        "AuditExt should see at least safe_tool"
    );

    // LifecycleExt uses EventBus (observational). Sleep to let events propagate.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        lifecycle.turn_end_count.load(Ordering::SeqCst) >= 2,
        "LifecycleExt should see multiple turn_end events, got {}",
        lifecycle.turn_end_count.load(Ordering::SeqCst)
    );
    assert_eq!(
        lifecycle.agent_end_count.load(Ordering::SeqCst), 1,
        "LifecycleExt should see exactly 1 agent_end"
    );
}

// ============================================================================
// Simple tool for persistence test
// ============================================================================

struct ReturnArgToolExt;

#[async_trait]
impl Extension for ReturnArgToolExt {
    fn name(&self) -> &str { "return_arg" }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "return_arg".to_string(),
            description: "Returns the provided value argument".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string", "description": "The value to return" }
                },
                "required": ["value"]
            }),
        }]
    }

    async fn execute_tool(
        &self, _tool_call_id: &str, params: serde_json::Value
    ) -> Result<AgentToolResult, AgentError> {
        let value = params.get("value").and_then(|v| v.as_str()).unwrap_or("");
        Ok(AgentToolResult {
            content: vec![Content::Text { text: format!("result: {}", value), text_signature: None }],
            details: None, is_error: false, terminate: false,
        })
    }
}

#[tokio::test]
async fn test_multi_extension_with_persistence() {
    let _ = tracing_subscriber::fmt().try_init();

    let store = Arc::new(MemoryStore::new());
    let tool_provider = Arc::new(ReturnArgToolExt);

    let manager = ExtensionManager::new(vec![tool_provider]);
    let (hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);

    let provider = TestProvider::sequence(vec![
        TestResponse::ToolCalls(vec![TestToolCall::new(
            "call_1",
            "return_arg",
            serde_json::json!({"value": "hello persistence"}),
        )]),
        TestResponse::Text("done".into()),
    ]);

    // First session: create, prompt, flush
    {
        let compaction_actor = make_compaction_actor(provider.clone());
        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "You are helpful.".into(),
            "test".to_string(),
            provider.clone(),
            Arc::new(hook_router),
            compaction_actor,
            tools,
            Some(store.clone()),
        vec![],
        );

        let results = session.prompt("call tool".to_string()).await.unwrap();
        assert!(!results.is_empty());
        session.flush().await.unwrap();
    }

    // Second session: restore from MemoryStore
    {
        let manager2 = ExtensionManager::new(vec![Arc::new(ReturnArgToolExt)]);
        let (hook_router2, handles2, _join_handles2) = manager2.spawn_all();
        let tools2 = manager2.collect_agent_tools(&handles2);

        let compaction_actor2 = make_compaction_actor(provider.clone());
        let mut session2 = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "You are helpful.".into(),
            "test".to_string(),
            provider,
            Arc::new(hook_router2),
            compaction_actor2,
            tools2,
            Some(store.clone()),
        vec![],
        );

        let restored = session2.restore().await.unwrap();
        assert!(restored > 0, "expected some entries to be restored");

        let msgs = session2.messages();
        assert!(
            msgs.iter().any(|m| {
                if let AgentMessage::ToolResult(tr) = m {
                    tr.content.iter().any(|c| matches!(c, Content::Text { text, .. } if text.contains("hello persistence")))
                } else {
                    false
                }
            }),
            "restored messages should contain tool execution results"
        );
    }
}
