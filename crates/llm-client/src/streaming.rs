use crate::types::{Api, AssistantMessage, Content, StopReason, ToolCall, Usage};

#[derive(Debug, Clone)]
pub enum AssistantMessageEvent {
    // ── 生命周期 ──
    Start {
        partial: AssistantMessage,
    },

    // ── 文本流（按 content_index 区分多个 text block）──
    TextStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    TextDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    TextEnd {
        content_index: usize,
        text: String,
        partial: AssistantMessage,
    },

    // ── 推理流（thinking / extended reasoning）──
    ThinkingStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    ThinkingDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    ThinkingEnd {
        content_index: usize,
        thinking: String,
        partial: AssistantMessage,
    },

    // ── 工具调用流（参数流式增量 + 最终 parsed ToolCall）──
    ToolCallStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    ToolCallDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    ToolCallEnd {
        content_index: usize,
        tool_call: ToolCall,
        partial: AssistantMessage,
    },

    // ── 终止事件 ──
    Done {
        reason: StopReason,
        message: AssistantMessage,
    },
    Error {
        error: AssistantMessage,
    },
}

impl AssistantMessageEvent {
    /// Create a minimal `Start` event with an empty partial.
    pub fn new_start(provider: &str, model: &str, response_id: Option<String>) -> Self {
        Self::Start {
            partial: AssistantMessage {
                content: Vec::new(),
                provider: provider.to_string(),
                model: model.to_string(),
                api: Api {
                    provider: provider.to_string(),
                    model: model.to_string(),
                },
                usage: Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: StopReason::Stop,
                response_id,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            },
        }
    }

    /// Create a `Done` event from accumulated content.
    pub fn new_done(
        content: Vec<Content>,
        api: Api,
        usage: Usage,
        stop_reason: StopReason,
        provider: &str,
        model: &str,
    ) -> Self {
        let mut usage = usage;
        usage.total_tokens = usage.compute_total();
        Self::Done {
            reason: stop_reason.clone(),
            message: AssistantMessage {
                content,
                provider: provider.to_string(),
                model: model.to_string(),
                api,
                usage,
                stop_reason,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            },
        }
    }
}

// ── AssistantMessageEventStream ──

use crate::error::LlmError;
use tokio::sync::mpsc;

pub struct AssistantMessageEventStream {
    rx: Option<mpsc::Receiver<AssistantMessageEvent>>,
    terminated: bool,
}

impl AssistantMessageEventStream {
    /// Create a new stream with internal channel.
    /// Returns (stream, sender) — sender is passed to the provider.
    pub fn new(buffer: usize) -> (Self, mpsc::Sender<AssistantMessageEvent>) {
        let (tx, rx) = mpsc::channel(buffer);
        (
            Self {
                rx: Some(rx),
                terminated: false,
            },
            tx,
        )
    }

    /// Await the next event. Returns None when the stream ends.
    pub async fn next(&mut self) -> Option<AssistantMessageEvent> {
        if self.terminated {
            return None;
        }
        match &mut self.rx {
            Some(rx) => {
                let event = rx.recv().await;
                if event.is_none() {
                    self.terminated = true;
                }
                event
            }
            None => None,
        }
    }

    /// Consume the stream and return the final AssistantMessage.
    ///
    /// Drains remaining events until Done or Error is received.
    /// - Done → Ok(message)
    /// - Error → Err(LlmError) with the error content
    /// - Stream ends without terminal event → Err(StreamError)
    pub async fn to_message(mut self) -> Result<AssistantMessage, LlmError> {
        while let Some(event) = self.next().await {
            match event {
                AssistantMessageEvent::Done { message, .. } => {
                    return Ok(message);
                }
                AssistantMessageEvent::Error { error } => {
                    return Err(LlmError::StreamError {
                        kind: crate::StreamErrorKind::Protocol,
                        message: error
                            .error_message
                            .unwrap_or_else(|| "stream terminated with error".to_string()),
                    });
                }
                _ => continue,
            }
        }
        Err(LlmError::StreamError {
            kind: crate::StreamErrorKind::Network,
            message: "stream ended without Done or Error".to_string(),
        })
    }

    /// Drain remaining events without processing. Used to ensure sender
    /// closure doesn't block. Drops the receiver explicitly.
    pub async fn drain(mut self) {
        while self.next().await.is_some() {}
        self.rx = None;
    }
}

// ── Convenience constructor for mock/testing ──

impl AssistantMessageEventStream {
    /// Create a stream from a vec of events (for testing/mocking only).
    /// Events are spawned into a tokio task and fed through the channel.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn from_events(events: Vec<AssistantMessageEvent>) -> Self {
        let (stream, tx) = Self::new(events.len().max(1));
        tokio::spawn(async move {
            for event in events {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        stream
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_partial(provider: &str, model: &str) -> AssistantMessage {
        AssistantMessage {
            content: Vec::new(),
            provider: provider.to_string(),
            model: model.to_string(),
            api: Api {
                provider: provider.to_string(),
                model: model.to_string(),
            },
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 0,
            },
            stop_reason: StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::UNIX_EPOCH,
        }
    }

    #[tokio::test]
    async fn test_event_stream_init_and_next() {
        let (mut stream, tx) = AssistantMessageEventStream::new(4);
        let partial = make_partial("test", "v1");

        tx.send(AssistantMessageEvent::Start {
            partial: partial.clone(),
        })
        .await
        .unwrap();
        drop(tx);

        let event = stream.next().await;
        assert!(matches!(event, Some(AssistantMessageEvent::Start { .. })));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_event_stream_done_flow() {
        let partial = make_partial("test", "v1");
        let events = vec![
            AssistantMessageEvent::Start {
                partial: partial.clone(),
            },
            AssistantMessageEvent::TextStart {
                content_index: 0,
                partial: partial.clone(),
            },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "Hello".to_string(),
                partial: partial.clone(),
            },
            AssistantMessageEvent::TextEnd {
                content_index: 0,
                text: "Hello".to_string(),
                partial: partial.clone(),
            },
            AssistantMessageEvent::Done {
                reason: StopReason::Stop,
                message: partial.clone(),
            },
        ];

        let mut stream = AssistantMessageEventStream::from_events(events);

        assert!(matches!(
            stream.next().await,
            Some(AssistantMessageEvent::Start { .. })
        ));
        assert!(matches!(
            stream.next().await,
            Some(AssistantMessageEvent::TextStart { .. })
        ));
        assert!(matches!(
            stream.next().await,
            Some(AssistantMessageEvent::TextDelta { .. })
        ));
        assert!(matches!(
            stream.next().await,
            Some(AssistantMessageEvent::TextEnd { .. })
        ));
        assert!(matches!(
            stream.next().await,
            Some(AssistantMessageEvent::Done { .. })
        ));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_event_stream_error_flow() {
        let partial = make_partial("test", "v1");
        let events = vec![
            AssistantMessageEvent::Start {
                partial: partial.clone(),
            },
            AssistantMessageEvent::Error {
                error: partial.clone(),
            },
        ];

        let mut stream = AssistantMessageEventStream::from_events(events);

        assert!(matches!(
            stream.next().await,
            Some(AssistantMessageEvent::Start { .. })
        ));
        assert!(matches!(
            stream.next().await,
            Some(AssistantMessageEvent::Error { .. })
        ));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_to_message_done() {
        let partial = make_partial("test", "v1");
        let events = vec![
            AssistantMessageEvent::Start {
                partial: partial.clone(),
            },
            AssistantMessageEvent::Done {
                reason: StopReason::Stop,
                message: partial.clone(),
            },
        ];

        let stream = AssistantMessageEventStream::from_events(events);
        let result = stream.to_message().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().stop_reason, StopReason::Stop);
    }

    #[tokio::test]
    async fn test_to_message_error() {
        let partial = make_partial("test", "v1");
        let events = vec![
            AssistantMessageEvent::Start {
                partial: partial.clone(),
            },
            AssistantMessageEvent::Error {
                error: AssistantMessage {
                    error_message: Some("bad".into()),
                    ..partial.clone()
                },
            },
        ];

        let stream = AssistantMessageEventStream::from_events(events);
        let result = stream.to_message().await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::LlmError::StreamError {
                kind: crate::StreamErrorKind::Protocol,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn test_to_message_no_terminal() {
        let events = vec![AssistantMessageEvent::Start {
            partial: make_partial("test", "v1"),
        }];

        let stream = AssistantMessageEventStream::from_events(events);
        let result = stream.to_message().await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::LlmError::StreamError {
                kind: crate::StreamErrorKind::Network,
                ..
            }
        ));
    }
}
