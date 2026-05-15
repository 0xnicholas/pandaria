use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ai_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, Content,
    LlmContext, LlmProvider, StopReason, StreamOptions, ToolCall, Usage,
};
use tokio_util::sync::CancellationToken;

/// Describes a single mock LLM response.
#[derive(Debug, Clone)]
pub enum TestResponse {
    /// Plain text response with `StopReason::Stop`.
    Text(String),
    /// One or more tool calls with `StopReason::ToolUse`.
    ToolCalls(Vec<TestToolCall>),
    /// Error response with `StopReason::Error`.
    Error(String),
    /// Context-overflow error (shorthand for `Error("context length exceeded")`).
    Overflow,
    /// Hang until the cancellation token fires, then return `LlmError::Cancelled`.
    Cancel,
}

/// Lightweight tool-call descriptor used by [`TestResponse::ToolCalls`].
#[derive(Debug, Clone)]
pub struct TestToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl TestToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: serde_json::Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }
}

/// A flexible mock [`LlmProvider`] for use in unit and integration tests.
///
/// # Examples
///
/// ```rust
/// use agent_core::test_utils::{TestProvider, TestResponse, TestToolCall};
///
/// // Static text response
/// let provider = TestProvider::text("Hello!");
///
/// // Sequence of responses
/// let provider = TestProvider::sequence(vec![
///     TestResponse::ToolCalls(vec![
///         TestToolCall::new("call_1", "read", serde_json::json!({"path": "/tmp"})),
///     ]),
///     TestResponse::Text("done".into()),
/// ]);
/// ```
pub struct TestProvider {
    factory: Arc<dyn Fn(usize) -> TestResponse + Send + Sync>,
    call_count: AtomicUsize,
}

impl TestProvider {
    /// Always returns the given text with `StopReason::Stop`.
    pub fn text(s: impl Into<String>) -> Arc<dyn LlmProvider> {
        let text = s.into();
        Arc::new(Self {
            factory: Arc::new(move |_| TestResponse::Text(text.clone())),
            call_count: AtomicUsize::new(0),
        })
    }

    /// Returns an error with the given message and `StopReason::Error`.
    pub fn error(msg: impl Into<String>) -> Arc<dyn LlmProvider> {
        let msg = msg.into();
        Arc::new(Self {
            factory: Arc::new(move |_| TestResponse::Error(msg.clone())),
            call_count: AtomicUsize::new(0),
        })
    }

    /// Returns a context-overflow error (`"context length exceeded"`).
    pub fn overflow() -> Arc<dyn LlmProvider> {
        Arc::new(Self {
            factory: Arc::new(|_| TestResponse::Overflow),
            call_count: AtomicUsize::new(0),
        })
    }

    /// Always returns the given tool call(s) with `StopReason::ToolUse`.
    pub fn tool_call(name: impl Into<String>, arguments: serde_json::Value) -> Arc<dyn LlmProvider> {
        let name = name.into();
        Arc::new(Self {
            factory: Arc::new(move |_| {
                TestResponse::ToolCalls(vec![TestToolCall {
                    id: format!("call_{}", uuid::Uuid::new_v4()),
                    name: name.clone(),
                    arguments: arguments.clone(),
                }])
            }),
            call_count: AtomicUsize::new(0),
        })
    }

    /// Hangs until the cancellation token fires, then returns `LlmError::Cancelled`.
    pub fn cancel() -> Arc<dyn LlmProvider> {
        Arc::new(Self {
            factory: Arc::new(|_| TestResponse::Cancel),
            call_count: AtomicUsize::new(0),
        })
    }

    /// Returns responses from the supplied vector in order, wrapping back to the
    /// start once exhausted.
    pub fn sequence(responses: Vec<TestResponse>) -> Arc<dyn LlmProvider> {
        let responses = Arc::new(responses);
        Arc::new(Self {
            factory: Arc::new(move |n| {
                responses.get(n).cloned().unwrap_or_else(|| {
                    // Once exhausted, default to empty Stop response to avoid
                    // infinite loops in simple tests.
                    TestResponse::Text("".into())
                })
            }),
            call_count: AtomicUsize::new(0),
        })
    }

    /// Calls the supplied closure with the current zero-based call index.
    pub fn counted<F>(factory: F) -> Arc<dyn LlmProvider>
    where
        F: Fn(usize) -> TestResponse + Send + Sync + 'static,
    {
        Arc::new(Self {
            factory: Arc::new(factory),
            call_count: AtomicUsize::new(0),
        })
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    fn build_message(&self,
        call_index: usize,
    ) -> (
            Vec<AssistantMessageEvent>,
            StopReason,
            Option<String>,
        ) {
        let response = (self.factory)(call_index);

        let (content, stop_reason, error_message) = match response {
            TestResponse::Text(text) => (
                vec![Content::Text { text, text_signature: None }],
                StopReason::Stop,
                None,
            ),
            TestResponse::ToolCalls(calls) => (
                calls
                    .into_iter()
                    .map(|c| {
                        Content::ToolCall(ToolCall {
                            id: c.id,
                            name: c.name,
                            arguments: c.arguments,
                            thought_signature: None,
                        })
                    })
                    .collect(),
                StopReason::ToolUse,
                None,
            ),
            TestResponse::Error(msg) => (vec![], StopReason::Error, Some(msg)),
            TestResponse::Overflow => (
                vec![],
                StopReason::Error,
                Some("context length exceeded".into()),
            ),
            TestResponse::Cancel => {
                // Cancel is handled directly in `stream()`; this arm should
                // never be reached when building events.
                return (vec![], StopReason::Stop, None);
            }
        };

        let partial = AssistantMessage {
            content: content.clone(),
            provider: "test".to_string(),
            model: "test".to_string(),
            api: ai_provider::Api {
                provider: "test".to_string(),
                model: "test".to_string(),
            },
            usage: Usage {
                input_tokens: 0,
                output_tokens: if stop_reason == StopReason::Stop { 1 } else { 0 },
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: if stop_reason == StopReason::Stop { 1 } else { 0 },
            },
            stop_reason: stop_reason.clone(),
            response_id: None,
            error_message: error_message.clone(),
            timestamp: std::time::SystemTime::now(),
        };

        let events = if stop_reason == StopReason::Error {
            vec![AssistantMessageEvent::Error {
                error: partial,
            }]
        } else {
            vec![
                AssistantMessageEvent::Start {
                    partial: partial.clone(),
                },
                AssistantMessageEvent::Done {
                    reason: stop_reason.clone(),
                    message: partial,
                },
            ]
        };

        (events, stop_reason, error_message)
    }
}

#[async_trait::async_trait]
impl LlmProvider for TestProvider {
    fn provider_name(&self) -> &str {
        "test"
    }

    fn models(&self) -> Vec<String> {
        vec!["test".to_string()]
    }

    fn config(&self) -> &ai_provider::providers::shared::ProviderConfig {
        use std::sync::OnceLock;
        static CONFIG: OnceLock<ai_provider::providers::shared::ProviderConfig> = OnceLock::new();
        CONFIG.get_or_init(|| {
            ai_provider::providers::shared::ProviderConfig::new(
                None,
                "http://test",
                "test",
                "TEST_API_KEY",
            )
        })
    }

    async fn stream(
        &self,
        _model: &str,
        _context: LlmContext,
        _options: StreamOptions,
        signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, ai_provider::LlmError> {
        let call_index = self.call_count.fetch_add(1, Ordering::SeqCst);

        // Special-case Cancel so that we block on the cancellation token.
        if let TestResponse::Cancel = (self.factory)(call_index) {
            let (_stream, _tx) = AssistantMessageEventStream::new(4);
            tokio::select! {
                _ = signal.cancelled() => {}
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(10)) => {}
            }
            return Err(ai_provider::LlmError::Cancelled);
        }

        let (stream, tx) = AssistantMessageEventStream::new(8);
        let (events, _stop_reason, _error_message) = self.build_message(call_index);

        tokio::spawn(async move {
            for event in events {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });

        Ok(stream)
    }
}

// ============================================================================
// Shared test dispatcher (replaces the dozens of inline `AllowAllDispatcher`s)
// ============================================================================

use crate::hook::dispatcher::HookDispatcher;

pub struct AllowAllDispatcher;

#[async_trait::async_trait]
impl HookDispatcher for AllowAllDispatcher {}
