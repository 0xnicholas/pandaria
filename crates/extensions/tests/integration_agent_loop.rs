use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use agent_core::context::{ContextCtx, ToolCallCtx, TurnEndCtx};
use agent_core::loop_::{AgentLoop, AgentLoopConfig};
use agent_core::mutations::{ContextMutation, HookDecision, ToolCallMutation};
use agent_core::types::{AgentMessage, AgentTool, AgentToolProgressUpdate, AgentToolRef, AgentToolResult};
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;
use llm_client::{Content, LlmContext, LlmProvider, StopReason, StreamOptions, ToolCall};

fn make_loop_config(
    provider: Arc<dyn LlmProvider>,
    hook_dispatcher: Arc<dyn agent_core::hook_dispatcher::HookDispatcher>,
    tools: Vec<AgentToolRef>,
    system_prompt: Option<String>,
) -> AgentLoopConfig {
    AgentLoopConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        model: "test".to_string(),
        provider,
        hook_dispatcher,
        tools,
        system_prompt,
        stream_options: StreamOptions::default(),
        max_retries: 3,
        steer_queue: Arc::new(Mutex::new(vec![])),
        follow_up_queue: Arc::new(Mutex::new(vec![])),
        event_sink: Arc::new(|event| {
            tracing::debug!("event: {:?}", event);
        }),
    }
}

// ============================================================================
// Mock LLM Provider
// ============================================================================

struct EchoProvider {
    content: Vec<Content>,
    stop_reason: StopReason,
}

#[async_trait]
impl LlmProvider for EchoProvider {
    fn provider_name(&self) -> &str { "echo" }
    fn models(&self) -> Vec<String> { vec!["echo".to_string()] }

    async fn stream(
        &self,
        _model: &str,
        _context: LlmContext,
        _options: StreamOptions,
        _signal: CancellationToken,
    ) -> Result<llm_client::AssistantMessageEventStream, llm_client::LlmError> {
        let (stream, tx) = llm_client::AssistantMessageEventStream::new(4);

        let partial = llm_client::AssistantMessage {
            content: self.content.clone(),
            provider: "echo".to_string(),
            model: "echo".to_string(),
            api: llm_client::Api { provider: "echo".to_string(), model: "echo".to_string() },
            usage: llm_client::Usage {
                input_tokens: 0, output_tokens: 0,
                cache_creation_input_tokens: None, cache_read_input_tokens: None,
                total_tokens: 0,
            },
            stop_reason: self.stop_reason.clone(),
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        };

        let events = vec![
            llm_client::AssistantMessageEvent::Start { partial: partial.clone() },
            llm_client::AssistantMessageEvent::Done { reason: self.stop_reason.clone(), message: partial },
        ];

        tokio::spawn(async move {
            for event in events {
                if tx.send(event).await.is_err() { break; }
            }
        });

        Ok(stream)
    }
}

struct ToolCallProvider {
    tool_calls: Vec<ToolCall>,
    call_count: AtomicUsize,
}

#[async_trait]
impl LlmProvider for ToolCallProvider {
    fn provider_name(&self) -> &str { "tool-call" }
    fn models(&self) -> Vec<String> { vec!["test".to_string()] }

    async fn stream(
        &self,
        _model: &str,
        _context: LlmContext,
        _options: StreamOptions,
        _signal: CancellationToken,
    ) -> Result<llm_client::AssistantMessageEventStream, llm_client::LlmError> {
        let (stream, tx) = llm_client::AssistantMessageEventStream::new(8);

        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        
        // Only return ToolUse on first call; return Stop on subsequent calls
        let (content, stop_reason) = if count == 0 {
            (
                self.tool_calls.iter().map(|tc| Content::ToolCall(tc.clone())).collect(),
                StopReason::ToolUse,
            )
        } else {
            (
                vec![Content::Text { text: "done".to_string(), text_signature: None }],
                StopReason::Stop,
            )
        };

        let partial = llm_client::AssistantMessage {
            content,
            provider: "tool-call".to_string(),
            model: "test".to_string(),
            api: llm_client::Api { provider: "tool-call".to_string(), model: "test".to_string() },
            usage: llm_client::Usage {
                input_tokens: 0, output_tokens: 0,
                cache_creation_input_tokens: None, cache_read_input_tokens: None,
                total_tokens: 0,
            },
            stop_reason: stop_reason.clone(),
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        };

        let events = vec![
            llm_client::AssistantMessageEvent::Start { partial: partial.clone() },
            llm_client::AssistantMessageEvent::Done { reason: stop_reason, message: partial },
        ];

        tokio::spawn(async move {
            for event in events {
                if tx.send(event).await.is_err() { break; }
            }
        });

        Ok(stream)
    }
}

// ============================================================================
// Mock Tool
// ============================================================================

struct EchoTool;

#[async_trait]
impl AgentTool for EchoTool {
    fn name(&self) -> &str { "echo_tool" }
    fn description(&self) -> &str { "Echoes input" }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
    ) -> Result<AgentToolResult, agent_core::error::AgentError> {
        Ok(AgentToolResult {
            content: vec![Content::Text { text: "echo_result".to_string(), text_signature: None }],
            details: None,
            is_error: false,
            terminate: false,
        })
    }
}

// ============================================================================
// Mock Extensions
// ============================================================================

struct BlockToolExt {
    target: String,
}

#[async_trait]
impl Extension for BlockToolExt {
    fn name(&self) -> &str { "tool_blocker" }

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        if ctx.tool_name == self.target {
            (HookDecision::Block { reason: format!("blocked: {}", self.target) }, ToolCallMutation::default())
        } else {
            (HookDecision::Continue, ToolCallMutation::default())
        }
    }
}

struct MutateContextExt {
    marker: String,
}

#[async_trait]
impl Extension for MutateContextExt {
    fn name(&self) -> &str { "context_mutator" }

    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
        ContextMutation {
            messages: Some(vec![AgentMessage::User(llm_client::UserMessage {
                content: vec![Content::Text {
                    text: self.marker.clone(),
                    text_signature: None,
                }],
                timestamp: std::time::SystemTime::now(),
            })]),
        }
    }
}

struct TurnEndCounterExt {
    count: AtomicUsize,
}

#[async_trait]
impl Extension for TurnEndCounterExt {
    fn name(&self) -> &str { "turn_counter" }

    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }
}

// ============================================================================
// Tests: AgentLoop + HookRouter integration
// ============================================================================

#[tokio::test]
async fn test_agent_loop_simple_with_router() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let router = HookRouter::new(vec![], bus);

    let provider = Arc::new(EchoProvider {
        content: vec![Content::Text { text: "Hello!".to_string(), text_signature: None }],
        stop_reason: StopReason::Stop,
    });

    let config = make_loop_config(provider, Arc::new(router), vec![], Some("You are helpful.".to_string()));
    let loop_ = AgentLoop::new(config);

    let user_msg = AgentMessage::User(llm_client::UserMessage {
        content: vec![Content::Text { text: "hi".to_string(), text_signature: None }],
        timestamp: std::time::SystemTime::now(),
    });

    let results = loop_.run(vec![user_msg], CancellationToken::new()).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(matches!(&results[0], AgentMessage::Assistant(_)));
}

#[tokio::test]
async fn test_agent_loop_context_mutation_via_extension() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(MutateContextExt { marker: "injected_by_ext".to_string() });
    let (handle, _) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);

    // Provider that verifies mutated context
    struct VerifyProvider;
    #[async_trait]
    impl LlmProvider for VerifyProvider {
        fn provider_name(&self) -> &str { "verify" }
        fn models(&self) -> Vec<String> { vec!["test".to_string()] }

        async fn stream(
            &self,
            _model: &str,
            context: LlmContext,
            _options: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<llm_client::AssistantMessageEventStream, llm_client::LlmError> {
            assert_eq!(context.messages.len(), 1);
            match &context.messages[0] {
                AgentMessage::User(user) => {
                    let text = user.content.first().and_then(|c| match c {
                        Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    });
                    assert_eq!(text, Some("injected_by_ext"));
                }
                _ => panic!("expected user message"),
            }

            let (stream, tx) = llm_client::AssistantMessageEventStream::new(4);
            let partial = llm_client::AssistantMessage {
                content: vec![Content::Text { text: "ok".to_string(), text_signature: None }],
                provider: "verify".to_string(),
                model: "test".to_string(),
                api: llm_client::Api { provider: "verify".to_string(), model: "test".to_string() },
                usage: llm_client::Usage {
                    input_tokens: 0, output_tokens: 0,
                    cache_creation_input_tokens: None, cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: StopReason::Stop,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            };

            tokio::spawn(async move {
                let _ = tx.send(llm_client::AssistantMessageEvent::Start { partial: partial.clone() }).await;
                let _ = tx.send(llm_client::AssistantMessageEvent::Done { reason: StopReason::Stop, message: partial }).await;
            });

            Ok(stream)
        }
    }

    let provider = Arc::new(VerifyProvider);
    let config = make_loop_config(provider, Arc::new(router), vec![], None);
    let loop_ = AgentLoop::new(config);

    let user_msg = AgentMessage::User(llm_client::UserMessage {
        content: vec![Content::Text { text: "original".to_string(), text_signature: None }],
        timestamp: std::time::SystemTime::now(),
    });

    let results = loop_.run(vec![user_msg], CancellationToken::new()).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_agent_loop_tool_blocked_by_extension() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(BlockToolExt { target: "echo_tool".to_string() });
    let (handle, _) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);

    let provider = Arc::new(ToolCallProvider {
        tool_calls: vec![
            ToolCall {
                id: "call_1".to_string(),
                name: "echo_tool".to_string(),
                arguments: serde_json::json!({}),
                thought_signature: None,
            },
        ],
        call_count: AtomicUsize::new(0),
    });

    let tool: AgentToolRef = Arc::new(EchoTool);
    let config = make_loop_config(provider, Arc::new(router), vec![tool], Some("You have tools.".to_string()));
    let loop_ = AgentLoop::new(config);

    let user_msg = AgentMessage::User(llm_client::UserMessage {
        content: vec![Content::Text { text: "call tool".to_string(), text_signature: None }],
        timestamp: std::time::SystemTime::now(),
    });

    let results = loop_.run(vec![user_msg], CancellationToken::new()).await.unwrap();

    // Should be: assistant + blocked tool_result + assistant(Stop) = 3 messages
    assert_eq!(results.len(), 3);
    assert!(matches!(&results[0], AgentMessage::Assistant(_)));
    assert!(matches!(&results[1], AgentMessage::ToolResult(_)));
    assert!(matches!(&results[2], AgentMessage::Assistant(_)));

    // Verify tool result shows blocked
    if let AgentMessage::ToolResult(tr) = &results[1] {
        assert!(tr.is_error);
        let details = tr.details.as_ref().unwrap();
        assert_eq!(details["blocked"], true);
    } else {
        panic!("expected tool result");
    }
}

#[tokio::test]
async fn test_agent_loop_turn_end_observed() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let counter = Arc::new(TurnEndCounterExt { count: AtomicUsize::new(0) });
    let (handle, _) = ExtensionActor::spawn(counter.clone(), bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus.clone());

    let provider = Arc::new(EchoProvider {
        content: vec![Content::Text { text: "Hello!".to_string(), text_signature: None }],
        stop_reason: StopReason::Stop,
    });

    let config = make_loop_config(provider, Arc::new(router), vec![], Some("You are helpful.".to_string()));
    let loop_ = AgentLoop::new(config);

    let user_msg = AgentMessage::User(llm_client::UserMessage {
        content: vec![Content::Text { text: "hi".to_string(), text_signature: None }],
        timestamp: std::time::SystemTime::now(),
    });

    let _results = loop_.run(vec![user_msg], CancellationToken::new()).await.unwrap();

    // Give EventBus handlers time to process
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(counter.count.load(Ordering::SeqCst), 1);
}
