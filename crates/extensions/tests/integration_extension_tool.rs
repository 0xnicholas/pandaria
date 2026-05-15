use std::sync::Arc;

use async_trait::async_trait;
use ai_provider::{Content, ToolDef};

use agent_core::compaction::{CompactionActor, CompactionConfig};
use agent_core::error::AgentError;
use agent_core::file_ops::DefaultFileOperationExtractor;
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

struct ReturnArgToolExt;

#[async_trait]
impl Extension for ReturnArgToolExt {
    fn name(&self) -> &str {
        "return_arg"
    }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "return_arg".to_string(),
            description: "Returns the provided value argument".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "value": {
                        "type": "string",
                        "description": "The value to return"
                    }
                },
                "required": ["value"]
            }),
        }]
    }

    async fn execute_tool(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
        let value = params
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(AgentToolResult {
            content: vec![Content::Text {
                text: format!("result: {}", value),
                text_signature: None,
            }],
            details: None,
            is_error: false,
            terminate: false,
        })
    }
}

struct UppercaseToolExt;

#[async_trait]
impl Extension for UppercaseToolExt {
    fn name(&self) -> &str {
        "uppercase"
    }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "uppercase".to_string(),
            description: "Converts the input value to uppercase".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "value": {
                        "type": "string",
                        "description": "The value to convert"
                    }
                },
                "required": ["value"]
            }),
        }]
    }

    async fn execute_tool(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
        let value = params
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(AgentToolResult {
            content: vec![Content::Text {
                text: value.to_uppercase(),
                text_signature: None,
            }],
            details: None,
            is_error: false,
            terminate: false,
        })
    }
}

struct SequentialToolExt;

#[async_trait]
impl Extension for SequentialToolExt {
    fn name(&self) -> &str {
        "sequential_tool"
    }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "sequential_tool".to_string(),
            description: "A tool that must run sequentially".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "value": {
                        "type": "string",
                        "description": "The value to process"
                    }
                },
                "required": ["value"]
            }),
        }]
    }

    fn tool_execution_modes(&self) -> std::collections::HashMap<String, agent_core::types::ToolExecutionMode> {
        let mut modes = std::collections::HashMap::new();
        modes.insert(
            "sequential_tool".to_string(),
            agent_core::types::ToolExecutionMode::Sequential,
        );
        modes
    }

    async fn execute_tool(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
        let value = params
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(AgentToolResult {
            content: vec![Content::Text {
                text: format!("sequential result: {}", value),
                text_signature: None,
            }],
            details: None,
            is_error: false,
            terminate: false,
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
async fn test_extension_tool_executes_via_actor() {
    let _ = tracing_subscriber::fmt().try_init();

    let ext = Arc::new(ReturnArgToolExt);
    let manager = ExtensionManager::new(vec![ext]);
    let (hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);

    let provider = TestProvider::sequence(vec![
        TestResponse::ToolCalls(vec![TestToolCall::new(
            "call_1",
            "return_arg",
            serde_json::json!({"value": "hello"}),
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
    assert_eq!(results.len(), 3, "expected 3 messages: assistant + tool_result + assistant");

    // First message: assistant with tool call
    assert!(matches!(&results[0], AgentMessage::Assistant(_)));

    // Second message: tool result
    if let AgentMessage::ToolResult(tr) = &results[1] {
        let text = tr
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(text, "result: hello");
    } else {
        panic!("expected tool result, got: {:?}", results[1]);
    }

    // Third message: assistant with stop
    assert!(matches!(&results[2], AgentMessage::Assistant(_)));
}

#[tokio::test]
async fn test_extension_tool_with_mutation() {
    let _ = tracing_subscriber::fmt().try_init();

    let ext = Arc::new(UppercaseToolExt);
    let manager = ExtensionManager::new(vec![ext]);
    let (hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);

    let provider = TestProvider::sequence(vec![
        TestResponse::ToolCalls(vec![TestToolCall::new(
            "call_1",
            "uppercase",
            serde_json::json!({"value": "hello world"}),
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
    assert_eq!(results.len(), 3, "expected 3 messages: assistant + tool_result + assistant");

    // First message: assistant with tool call
    assert!(matches!(&results[0], AgentMessage::Assistant(_)));

    // Second message: tool result
    if let AgentMessage::ToolResult(tr) = &results[1] {
        let text = tr
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(text, "HELLO WORLD");
    } else {
        panic!("expected tool result, got: {:?}", results[1]);
    }

    // Third message: assistant with stop
    assert!(matches!(&results[2], AgentMessage::Assistant(_)));
}

#[tokio::test]
async fn test_extension_tool_execution_mode_configurable() {
    let ext = Arc::new(SequentialToolExt);
    let manager = ExtensionManager::new(vec![ext]);
    let (_hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name(), "sequential_tool");
    assert_eq!(
        tools[0].execution_mode(),
        agent_core::types::ToolExecutionMode::Sequential,
        "sequential_tool should be configured as Sequential"
    );
}
