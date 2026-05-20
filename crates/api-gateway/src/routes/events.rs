use axum::{
    extract::{Extension, Path, State},
    http::HeaderMap,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    error::GatewayError,
    middleware::TenantId,
    sse::SseStream,
    types::{ServerEvent, UsageInfo},
};
use crate::server::AppState;

pub async fn stream(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<SseStream, GatewayError> {
    // Validate Accept header
    if let Some(accept) = headers.get("accept").and_then(|v| v.to_str().ok()) {
        if !accept.contains("text/event-stream") && !accept.contains("*/*") {
            return Err(GatewayError::NotAcceptable);
        }
    }
    let mut rx = state
        .tenant_manager
        .subscribe_events(&tenant_id.0, &id)
        .await?;

    let (sse_tx, sse_rx) = tokio::sync::mpsc::channel::<ServerEvent>(256);

    let handle = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Some(server_event) = map_agent_event(event) {
                if sse_tx.send(server_event).await.is_err() {
                    break;
                }
            }
        }
    });

    Ok(SseStream::new(sse_rx, handle.abort_handle()))
}

fn map_agent_event(event: agent_core::AgentEvent) -> Option<ServerEvent> {
    use agent_core::AgentEvent;

    match event {
        AgentEvent::MessageStart { message_index } => {
            Some(ServerEvent::MessageStart { message_index })
        }
        AgentEvent::MessageUpdate { content_delta, .. } => {
            Some(ServerEvent::TextDelta { delta: content_delta })
        }
        AgentEvent::ToolExecutionEnd { tool_call_id, result } => {
            let result_text = extract_tool_result_text(&result.content);
            Some(ServerEvent::ToolCallDone {
                call_id: tool_call_id,
                result: result_text,
                is_error: result.is_error,
            })
        }
        AgentEvent::TurnEnd { messages, .. } => {
            let (stop_reason, usage) = extract_turn_end_info(&messages);
            Some(ServerEvent::TurnEnd {
                stop_reason,
                usage,
            })
        }
        AgentEvent::Error { error } => Some(ServerEvent::Error {
            code: error_variant_name(&error),
            message: error.to_sanitized_string(),
        }),
        // MVP 不转发的事件
        AgentEvent::AgentStart
        | AgentEvent::AgentEnd { .. }
        | AgentEvent::TurnStart { .. }
        | AgentEvent::MessageEnd { .. }
        | AgentEvent::ToolExecutionStart { .. }
        | AgentEvent::ToolExecutionUpdate { .. }
        | AgentEvent::CompactionStart { .. }
        | AgentEvent::CompactionEnd { .. }
        | AgentEvent::AutoRetryStart { .. }
        | AgentEvent::AutoRetryEnd { .. } => None,
        // Forward-compatibility for future AgentEvent variants
        _ => {
            tracing::warn!("unhandled AgentEvent variant in SSE mapper");
            None
        }
    }
}

fn extract_tool_result_text(contents: &[agent_core::Content]) -> Option<String> {
    let parts: Vec<String> = contents
        .iter()
        .map(|c| match c {
            agent_core::Content::Text { text, .. } => text.clone(),
            agent_core::Content::Image { mime_type, .. } => format!("[image: {}]", mime_type),
            agent_core::Content::Video { mime_type, .. } => format!("[video: {}]", mime_type),
            agent_core::Content::Audio { mime_type, .. } => format!("[audio: {}]", mime_type),
            _ => String::new(),
        })
        .collect();
    let joined = parts.join("\n");
    if joined.is_empty() { None } else { Some(joined) }
}

fn extract_turn_end_info(
    messages: &[agent_core::AgentMessage],
) -> (String, Option<UsageInfo>) {
    let last_assistant = messages.iter().rev().find_map(|m| match m {
        agent_core::AgentMessage::Assistant(a) => Some(a),
        _ => None,
    });

    match last_assistant {
        Some(a) => {
            let stop_reason = format!("{:?}", a.stop_reason).to_lowercase();
            let usage = Some(UsageInfo {
                input_tokens: a.usage.input_tokens,
                output_tokens: a.usage.output_tokens,
            });
            (stop_reason, usage)
        }
        None => ("unknown".into(), None),
    }
}

fn error_variant_name(error: &agent_core::AgentError) -> String {
    error.code().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tool_result_text() {
        let contents = vec![
            agent_core::Content::Text {
                text: "hello".into(),
                text_signature: None,
            },
            agent_core::Content::Text {
                text: " world".into(),
                text_signature: None,
            },
        ];
        assert_eq!(extract_tool_result_text(&contents), Some("hello\n world".into()));
    }

    #[test]
    fn test_extract_tool_result_text_with_media() {
        let contents = vec![
            agent_core::Content::Text {
                text: "result".into(),
                text_signature: None,
            },
            agent_core::Content::Image {
                data: "base64".into(),
                mime_type: "image/png".into(),
            },
        ];
        assert_eq!(
            extract_tool_result_text(&contents),
            Some("result\n[image: image/png]".into())
        );
    }

    #[test]
    fn test_extract_tool_result_text_empty() {
        let contents: Vec<agent_core::Content> = vec![];
        assert_eq!(extract_tool_result_text(&contents), None);
    }

    #[test]
    fn test_error_variant_name() {
        let err = agent_core::AgentError::ContextOverflow("test".into());
        assert_eq!(error_variant_name(&err), "context_overflow");

        let err = agent_core::AgentError::LlmError(ai_provider::LlmError::ProviderError("test".into()));
        assert_eq!(error_variant_name(&err), "llm_error");
    }
}
