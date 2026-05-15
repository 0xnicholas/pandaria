use std::sync::Arc;

use async_trait::async_trait;
use ai_provider::{Content, ToolDef};

use agent_core::compaction::{CompactionActor, CompactionConfig};
use agent_core::context::ToolCallCtx;
use agent_core::error::AgentError;
use agent_core::file_ops::DefaultFileOperationExtractor;
use agent_core::mutations::{HookDecision, ToolCallMutation};
use agent_core::SessionActor;
use agent_core::test_utils::{TestProvider, TestResponse, TestToolCall};
use agent_core::types::{AgentMessage, AgentToolResult};
use extensions::host::extension::Extension;
use extensions::host::manager::ExtensionManager;

fn make_compaction_actor(provider: Arc<dyn ai_provider::LlmProvider>) -> Arc<CompactionActor> {
    Arc::new(CompactionActor::new(
        CompactionConfig::default(),
        provider,
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    ))
}

// ============================================================================
// Mock Extensions
// ============================================================================

struct ToolProviderExt;

#[async_trait]
impl Extension for ToolProviderExt {
    fn name(&self) -> &str {
        "tool_provider"
    }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "sensitive_tool".to_string(),
            description: "A sensitive tool".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "secret": { "type": "string" }
                }
            }),
        }]
    }

    async fn execute_tool(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
        let secret = params.get("secret").and_then(|v| v.as_str()).unwrap_or("");
        let sanitized = params.get("sanitized").and_then(|v| v.as_bool()).unwrap_or(false);
        Ok(AgentToolResult {
            content: vec![Content::Text {
                text: format!("processed: secret={}, sanitized={}", secret, sanitized),
                text_signature: None,
            }],
            details: None,
            is_error: false,
            terminate: false,
        })
    }
}

struct InputSanitizerExt;

#[async_trait]
impl Extension for InputSanitizerExt {
    fn name(&self) -> &str {
        "input_sanitizer"
    }

    async fn on_tool_call(
        &self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        let mut input = ctx.input.clone();
        if let Some(obj) = input.as_object_mut() {
            obj.remove("secret");
            obj.insert("sanitized".to_string(), serde_json::json!(true));
        }
        (HookDecision::Continue, ToolCallMutation { input: Some(input) })
    }
}

struct ToolBlockerExt {
    target: String,
}

#[async_trait]
impl Extension for ToolBlockerExt {
    fn name(&self) -> &str {
        "tool_blocker"
    }

    async fn on_tool_call(
        &self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        if ctx.tool_name == self.target {
            (
                HookDecision::Block {
                    reason: format!("blocked: {}", self.target),
                },
                ToolCallMutation::default(),
            )
        } else {
            (HookDecision::Continue, ToolCallMutation::default())
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
async fn test_extension_tool_blocked_by_another_extension() {
    let _ = tracing_subscriber::fmt().try_init();

    let tool_provider = Arc::new(ToolProviderExt);
    let tool_blocker = Arc::new(ToolBlockerExt {
        target: "sensitive_tool".to_string(),
    });

    let manager = ExtensionManager::new(vec![tool_provider, tool_blocker]);
    let (hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);

    let provider = TestProvider::sequence(vec![
        TestResponse::ToolCalls(vec![TestToolCall::new(
            "call_1",
            "sensitive_tool",
            serde_json::json!({}),
        )]),
        TestResponse::Text("done".into()),
    ]);

    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".to_string(),
        "test".to_string(),
        provider,
        Arc::new(hook_router),
        compaction_actor,
        tools,
        None,
    vec![],
    );

    let results = session.prompt("call tool".to_string()).await.unwrap();
    assert_eq!(
        results.len(),
        3,
        "expected 3 messages: assistant + tool_result + assistant"
    );

    // First message: assistant with tool call
    assert!(matches!(&results[0], AgentMessage::Assistant(_)));

    // Second message: tool result should show blocked
    if let AgentMessage::ToolResult(tr) = &results[1] {
        assert!(
            tr.is_error,
            "blocked tool should have is_error=true, got: {:?}",
            tr
        );
        let details = tr.details.as_ref().expect("should have details");
        assert_eq!(details["blocked"], true);
        assert_eq!(details["reason"], "blocked: sensitive_tool");
    } else {
        panic!("expected tool result, got: {:?}", results[1]);
    }

    // Third message: assistant with stop
    assert!(matches!(&results[2], AgentMessage::Assistant(_)));
}

#[tokio::test]
async fn test_extension_tool_allowed_after_sanitization() {
    let _ = tracing_subscriber::fmt().try_init();

    let tool_provider = Arc::new(ToolProviderExt);
    let sanitizer = Arc::new(InputSanitizerExt);

    let manager = ExtensionManager::new(vec![tool_provider, sanitizer]);
    let (hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);

    let provider = TestProvider::sequence(vec![
        TestResponse::ToolCalls(vec![TestToolCall::new(
            "call_1",
            "sensitive_tool",
            serde_json::json!({"secret": "password123"}),
        )]),
        TestResponse::Text("done".into()),
    ]);

    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".to_string(),
        "test".to_string(),
        provider,
        Arc::new(hook_router),
        compaction_actor,
        tools,
        None,
    vec![],
    );

    let results = session.prompt("call tool".to_string()).await.unwrap();
    assert_eq!(
        results.len(),
        3,
        "expected 3 messages: assistant + tool_result + assistant"
    );

    // First message: assistant with tool call
    assert!(matches!(&results[0], AgentMessage::Assistant(_)));

    // Second message: tool result should show sanitized execution
    if let AgentMessage::ToolResult(tr) = &results[1] {
        assert!(
            !tr.is_error,
            "sanitized tool should have is_error=false, got: {:?}",
            tr
        );
        let text = tr
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(
            text, "processed: secret=, sanitized=true",
            "tool should receive sanitized input (no secret, sanitized=true)"
        );
    } else {
        panic!("expected tool result, got: {:?}", results[1]);
    }

    // Third message: assistant with stop
    assert!(matches!(&results[2], AgentMessage::Assistant(_)));
}
