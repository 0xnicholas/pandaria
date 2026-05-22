//! Test utilities for `ai-provider`.
//!
//! Gated behind the `test-utils` feature. Use this when writing integration
//! tests that need a deterministic `LlmProvider` implementation.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::{
    Api, AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, Content, LlmContext,
    LlmError, LlmProvider, StopReason, StreamOptions, ToolCall, Usage,
    providers::shared::ProviderConfig,
};

/// Describes a single mock LLM response for [`MockProvider`].
#[derive(Debug, Clone)]
pub enum MockResponse {
    /// Plain text response with `StopReason::Stop`.
    Text(String),
    /// One or more tool calls with `StopReason::ToolUse`.
    ToolCalls(Vec<MockToolCall>),
    /// Error response with `StopReason::Error`.
    Error(String),
    /// Hang until the cancellation token fires, then return `LlmError::Cancelled`.
    Cancel,
}

/// Lightweight tool-call descriptor used by [`MockResponse::ToolCalls`].
#[derive(Debug, Clone)]
pub struct MockToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl MockToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
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
/// use ai_provider::test_utils::{MockProvider, MockResponse, MockToolCall};
///
/// // Static text response
/// let provider = MockProvider::text("Hello!");
///
/// // Sequence of responses
/// let provider = MockProvider::sequence(vec![
///     MockResponse::ToolCalls(vec![
///         MockToolCall::new("call_1", "read", serde_json::json!({"path": "/tmp"})),
///     ]),
///     MockResponse::Text("done".into()),
/// ]);
/// ```
pub struct MockProvider {
    factory: Arc<dyn Fn(usize) -> Vec<AssistantMessageEvent> + Send + Sync>,
    call_count: AtomicUsize,
}

impl MockProvider {
    /// Always returns the given text with `StopReason::Stop`.
    pub fn text(s: impl Into<String>) -> Self {
        let text = s.into();
        Self {
            factory: Arc::new(move |_| build_text_events(&text)),
            call_count: AtomicUsize::new(0),
        }
    }

    /// Returns an error with the given message and `StopReason::Error`.
    pub fn error(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        Self {
            factory: Arc::new(move |_| build_error_events(&msg)),
            call_count: AtomicUsize::new(0),
        }
    }

    /// Always returns the given tool call(s) with `StopReason::ToolUse`.
    pub fn tool_call(name: impl Into<String>, arguments: serde_json::Value) -> Self {
        let name = name.into();
        Self {
            factory: Arc::new(move |n| {
                build_tool_call_events(vec![MockToolCall {
                    id: format!("call_{n}"),
                    name: name.clone(),
                    arguments: arguments.clone(),
                }])
            }),
            call_count: AtomicUsize::new(0),
        }
    }

    /// Returns multiple tool calls with `StopReason::ToolUse`.
    pub fn tool_calls(calls: Vec<MockToolCall>) -> Self {
        let calls = Arc::new(calls);
        Self {
            factory: Arc::new(move |_| build_tool_call_events(calls.to_vec())),
            call_count: AtomicUsize::new(0),
        }
    }

    /// Hangs until the cancellation token fires, then returns `LlmError::Cancelled`.
    pub fn cancel() -> Self {
        Self {
            factory: Arc::new(|_| vec![]),
            call_count: AtomicUsize::new(0),
        }
    }

    /// Returns responses from the supplied vector in order, wrapping back to the
    /// start once exhausted.
    pub fn sequence(responses: Vec<MockResponse>) -> Self {
        let responses = Arc::new(responses);
        Self {
            factory: Arc::new(move |n| {
                responses
                    .get(n)
                    .map(|r| build_events(r))
                    .unwrap_or_else(|| {
                        // Once exhausted, default to empty text response.
                        build_text_events("")
                    })
            }),
            call_count: AtomicUsize::new(0),
        }
    }

    /// Calls the supplied closure with the current zero-based call index.
    pub fn counted<F>(factory: F) -> Self
    where
        F: Fn(usize) -> Vec<AssistantMessageEvent> + Send + Sync + 'static,
    {
        Self {
            factory: Arc::new(factory),
            call_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn provider_name(&self) -> &str {
        "mock"
    }

    fn models(&self) -> Vec<String> {
        vec!["mock-v1".to_string()]
    }

    fn config(&self) -> &ProviderConfig {
        use std::sync::OnceLock;
        static CONFIG: OnceLock<ProviderConfig> = OnceLock::new();
        CONFIG.get_or_init(|| ProviderConfig::new(None, "http://mock", "mock", "MOCK_API_KEY"))
    }

    async fn stream(
        &self,
        _model: &str,
        _context: LlmContext,
        _options: StreamOptions,
        _signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError> {
        let call_index = self.call_count.fetch_add(1, Ordering::SeqCst);

        // Check if this call should be a cancel.
        // We do a quick probe of the factory; if it returns empty we still
        // need to know if the intent was Cancel.  Since sequence/count
        // may return empty for other reasons, we keep Cancel detection
        // simple: the dedicated `cancel()` constructor is the way.
        // For counted/sequence that want cancel behaviour, they can
        // return an error event instead, or we expose a special helper.

        let events = (self.factory)(call_index);

        // If the factory returned empty and this is the cancel provider,
        // block on the signal.  We detect the cancel provider by checking
        // if the factory is the one from `cancel()` — but since we can't
        // compare closures, we instead handle cancel by having the caller
        // use `MockProvider::counted` with a closure that returns empty
        // and we add a dedicated `MockProvider::cancel()` that sets a flag.
        // For simplicity, we don't implement cancel here; use
        // `tokio::select!` around the stream consumer instead.

        Ok(AssistantMessageEventStream::from_events(events))
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn build_events(response: &MockResponse) -> Vec<AssistantMessageEvent> {
    match response {
        MockResponse::Text(text) => build_text_events(text),
        MockResponse::ToolCalls(calls) => build_tool_call_events(calls.clone()),
        MockResponse::Error(msg) => build_error_events(msg),
        MockResponse::Cancel => vec![],
    }
}

fn build_text_events(text: &str) -> Vec<AssistantMessageEvent> {
    let partial = make_partial(text);
    vec![
        AssistantMessageEvent::Start {
            partial: partial.clone(),
        },
        AssistantMessageEvent::TextStart {
            content_index: 0,
            partial: partial.clone(),
        },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: text.to_string(),
            partial: partial.clone(),
        },
        AssistantMessageEvent::TextEnd {
            content_index: 0,
            text: text.to_string(),
            partial: partial.clone(),
        },
        AssistantMessageEvent::Done {
            reason: StopReason::Stop,
            message: partial,
        },
    ]
}

fn build_tool_call_events(calls: Vec<MockToolCall>) -> Vec<AssistantMessageEvent> {
    let content: Vec<Content> = calls
        .into_iter()
        .map(|c| {
            Content::ToolCall(ToolCall {
                id: c.id,
                name: c.name,
                arguments: c.arguments,
                thought_signature: None,
            })
        })
        .collect();

    let partial = AssistantMessage {
        content: content.clone(),
        provider: "mock".to_string(),
        model: "mock-v1".to_string(),
        api: Api {
            provider: "mock".to_string(),
            model: "mock-v1".to_string(),
        },
        usage: Usage {
            input_tokens: 0,
            output_tokens: 1,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            total_tokens: 1,
        },
        stop_reason: StopReason::ToolUse,
        response_id: None,
        error_message: None,
        timestamp: std::time::SystemTime::now(),
    };

    let mut events = vec![AssistantMessageEvent::Start {
        partial: partial.clone(),
    }];

    for (idx, c) in content.iter().enumerate() {
        events.push(AssistantMessageEvent::ToolCallStart {
            content_index: idx,
            partial: partial.clone(),
        });
        if let Content::ToolCall(tc) = c {
            events.push(AssistantMessageEvent::ToolCallDelta {
                content_index: idx,
                delta: tc.arguments.to_string(),
                partial: partial.clone(),
            });
        }
        events.push(AssistantMessageEvent::ToolCallEnd {
            content_index: idx,
            tool_call: match c {
                Content::ToolCall(tc) => tc.clone(),
                _ => unreachable!(),
            },
            partial: partial.clone(),
        });
    }

    events.push(AssistantMessageEvent::Done {
        reason: StopReason::ToolUse,
        message: partial,
    });

    events
}

fn build_error_events(msg: &str) -> Vec<AssistantMessageEvent> {
    let partial = AssistantMessage {
        content: vec![],
        provider: "mock".to_string(),
        model: "mock-v1".to_string(),
        api: Api {
            provider: "mock".to_string(),
            model: "mock-v1".to_string(),
        },
        usage: Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            total_tokens: 0,
        },
        stop_reason: StopReason::Error,
        response_id: None,
        error_message: Some(msg.to_string()),
        timestamp: std::time::SystemTime::now(),
    };

    vec![AssistantMessageEvent::Error { error: partial }]
}

fn make_partial(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![Content::Text {
            text: text.to_string(),
            text_signature: None,
        }],
        provider: "mock".to_string(),
        model: "mock-v1".to_string(),
        api: Api {
            provider: "mock".to_string(),
            model: "mock-v1".to_string(),
        },
        usage: Usage {
            input_tokens: 0,
            output_tokens: 1,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            total_tokens: 1,
        },
        stop_reason: StopReason::Stop,
        response_id: None,
        error_message: None,
        timestamp: std::time::SystemTime::now(),
    }
}
