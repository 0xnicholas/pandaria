//! End-to-end tool-use integration tests: verify that AgentLoop correctly
//! identifies `StopReason::ToolUse`, executes the tool via ToolExecutor,
//! formats the result as a `ToolResultMessage`, and sends it back in the
//! next LLM request.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_core::{
    AgentTool, AgentToolProgressUpdate, AgentToolRef, AgentToolResult, CompactionActor,
    CompactionConfig, DefaultFileOperationExtractor, SessionActor, SessionConfig,
};
use agent_core::test_utils::AllowAllDispatcher;
use agent_core::types::AgentMessage;
use ai_provider::{
    Content, LlmProvider,
    providers::openai::OpenAiProvider,
};
use async_trait::async_trait;
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_compaction_actor(provider: Arc<dyn LlmProvider>) -> Arc<CompactionActor> {
    Arc::new(CompactionActor::new(
        CompactionConfig::default(),
        provider,
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    ))
}

// ============================================================================
// Simple echo tool for testing
// ============================================================================

struct EchoTool;

#[async_trait]
impl AgentTool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echo the input message back."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string", "description": "message to echo" }
            },
            "required": ["message"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
    ) -> Result<AgentToolResult, agent_core::error::AgentError> {
        let msg = params["message"].as_str().unwrap_or("?");
        Ok(AgentToolResult {
            content: vec![Content::Text {
                text: msg.to_string(),
                text_signature: None,
            }],
            details: None,
            is_error: false,
            terminate: false,
        })
    }
}

// ============================================================================
// Full tool-use loop via OpenAI provider
// ============================================================================

#[tokio::test]
async fn test_tool_use_roundtrip_with_openai_provider() {
    let _ = tracing_subscriber::fmt().try_init();

    let server = MockServer::start().await;

    // Turn 1: assistant emits a tool call
    let turn1_body = r#"data: {"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"echo"}}]},"index":0}]}

data: {"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"message\":"}}]},"index":0}]}

data: {"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"hello\"}"}}]},"index":0}]}

data: {"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"tool_calls","index":0}]}

data: [DONE]

"#;

    // Turn 2: assistant responds after receiving tool result
    let turn2_body = r#"data: {"id":"chatcmpl-t2","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-t2","object":"chat.completion.chunk","choices":[{"delta":{"content":"Done"},"index":0}]}

data: {"id":"chatcmpl-t2","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();
    let turn2_has_tool_result = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let turn2_has_tool_result_clone = turn2_has_tool_result.clone();

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let count = call_count_clone.fetch_add(1, Ordering::SeqCst);
            if count == 1 {
                // Second call should contain the tool_result message
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                let messages = body["messages"].as_array().unwrap();
                let has_tool_result = messages.iter().any(|m| {
                    m["role"] == "tool" && m["tool_call_id"] == "call_abc"
                });
                turn2_has_tool_result_clone.store(has_tool_result, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_string(turn2_body)
            } else {
                ResponseTemplate::new(200).set_body_string(turn1_body)
            }
        })
        .mount(&server)
        .await;

    let provider: Arc<dyn LlmProvider> = Arc::new(
        OpenAiProvider::with_base_url(
            Some(SecretString::new("sk-test".into())),
            &server.uri(),
        )
    );

    let tools: Vec<AgentToolRef> = vec![Arc::new(EchoTool)];

    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "gpt-5.2".to_string(),
        provider: provider.clone(),
        hook_dispatcher: Arc::new(AllowAllDispatcher),
        compaction_actor: make_compaction_actor(provider),
        tools,
        store: None,
        skills: vec![],
    });

    let results = session.prompt("Call the echo tool with hello".to_string()).await.unwrap();

    // We expect 3 messages: user, assistant (tool call), assistant (final text)
    // Actually prompt() returns all new messages generated in this turn.
    // The first turn produces: assistant message with tool call
    // Then tool is executed, and a second LLM call is made
    // The second turn produces: assistant message with text "Done"
    // So results should contain 2 assistant messages? Or the tool result is not included?
    // Let me check prompt() return value...
    // From session.rs, prompt() returns the result of agent_loop.run(), which is Vec<AgentMessage>
    // It includes all new messages added in the turn (assistant + any follow-ups)
    // But tool results are added to the internal messages list, not returned?

    // Let's just verify the second request had the tool result.
    assert!(
        turn2_has_tool_result.load(Ordering::SeqCst),
        "second LLM request should contain the tool_result message"
    );

    // Verify we had 2 LLM calls (tool call + final response)
    assert_eq!(call_count.load(Ordering::SeqCst), 2);

    // Verify entries contain the full conversation
    let entries = session.entries();
    assert!(
        entries.len() >= 4,
        "expected at least 4 entries: user, assistant(tool), tool_result, assistant(text)"
    );
}
