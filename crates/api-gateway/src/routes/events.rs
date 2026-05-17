use axum::{
    extract::{Extension, Path, State},
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
) -> Result<SseStream, GatewayError> {
    let mut rx = state
        .tenant_manager
        .subscribe_events(&tenant_id.0, &id)
        .await?;

    let (sse_tx, sse_rx) = tokio::sync::mpsc::channel::<ServerEvent>(256);

    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Some(server_event) = map_agent_event(event) {
                if sse_tx.send(server_event).await.is_err() {
                    break;
                }
            }
        }
    });

    Ok(SseStream::new(sse_rx))
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
            let result_text = extract_text_content(&result.content);
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
            message: error.to_string(),
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
    }
}

fn extract_text_content(contents: &[agent_core::Content]) -> Option<String> {
    let texts: Vec<String> = contents
        .iter()
        .filter_map(|c| match c {
            agent_core::Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();

    if texts.is_empty() {
        None
    } else {
        Some(texts.join(""))
    }
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
            let usage = if a.usage.input_tokens > 0 || a.usage.output_tokens > 0 {
                Some(UsageInfo {
                    input_tokens: a.usage.input_tokens,
                    output_tokens: a.usage.output_tokens,
                })
            } else {
                None
            };
            (stop_reason, usage)
        }
        None => ("unknown".into(), None),
    }
}

fn error_variant_name(error: &agent_core::AgentError) -> String {
    let full = format!("{:?}", error);
    full.split_once('(')
        .map(|(name, _)| name.to_snake_case())
        .unwrap_or_else(|| "unknown".into())
}

/// 简单的 snake_case 转换辅助函数。
pub trait ToSnakeCase {
    fn to_snake_case(&self) -> String;
}

impl ToSnakeCase for str {
    fn to_snake_case(&self) -> String {
        let mut result = String::with_capacity(self.len() + 4);
        let chars: Vec<char> = self.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            if c.is_uppercase() {
                if i > 0 && chars[i - 1].is_lowercase() {
                    result.push('_');
                }
                result.push(c.to_lowercase().next().expect("uppercase char has at least one lowercase counterpart"));
            } else {
                result.push(*c);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_content() {
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
        assert_eq!(extract_text_content(&contents), Some("hello world".into()));
    }

    #[test]
    fn test_extract_text_content_empty() {
        let contents: Vec<agent_core::Content> = vec![];
        assert_eq!(extract_text_content(&contents), None);
    }

    #[test]
    fn test_snake_case() {
        assert_eq!("ContextOverflow".to_snake_case(), "context_overflow");
        assert_eq!("LlmError".to_snake_case(), "llm_error");
    }
}
