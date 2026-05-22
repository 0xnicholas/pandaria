use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    http::StatusCode,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::server::AppState;
use crate::{
    error::GatewayError,
    middleware::TenantId,
    types::{MessageContentPart, SendMessageRequest, UsageInfo},
};

#[derive(Debug, serde::Deserialize)]
pub struct SendMessageQuery {
    #[serde(default)]
    pub wait: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    30000
}

/// 发送消息并阻塞直到当前 turn 完成。
/// 客户端应在调用此端点**之前**先订阅 `/events` SSE，否则可能错过事件。
fn convert_content(parts: Vec<MessageContentPart>) -> Vec<ai_provider::Content> {
    parts
        .into_iter()
        .map(|p| match p {
            MessageContentPart::Text { text } => ai_provider::Content::Text {
                text,
                text_signature: None,
            },
            MessageContentPart::Image { data, mime_type } => {
                ai_provider::Content::Image { data, mime_type }
            }
            MessageContentPart::Video { data, mime_type } => {
                ai_provider::Content::Video { data, mime_type }
            }
            MessageContentPart::Audio { data, mime_type } => {
                ai_provider::Content::Audio { data, mime_type }
            }
        })
        .collect()
}

pub async fn send(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
    Query(query): Query<SendMessageQuery>,
    Json(req): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), GatewayError> {
    let content = convert_content(req.content);

    if query.wait {
        let result = state
            .tenant_manager
            .send_message_and_wait(&tenant_id.0, &id, content, query.timeout_ms)
            .await?;

        match result {
            tenant::WaitResult::Completed {
                turn_index,
                messages,
            } => {
                let last_assistant = messages.iter().rev().find_map(|m| match m {
                    agent_core::AgentMessage::Assistant(a) => Some(a),
                    _ => None,
                });
                let usage = last_assistant.map(|a| UsageInfo {
                    input_tokens: a.usage.input_tokens,
                    output_tokens: a.usage.output_tokens,
                });

                let body = serde_json::json!({
                    "turn_index": turn_index,
                    "completed": true,
                    "messages": messages,
                    "usage": usage,
                });
                Ok((StatusCode::OK, Json(body)))
            }
            tenant::WaitResult::Timeout { turn_index } => {
                let body = serde_json::json!({
                    "turn_index": turn_index,
                    "completed": false,
                    "message": "turn still in progress, subscribe to events for updates",
                });
                Ok((StatusCode::ACCEPTED, Json(body)))
            }
        }
    } else {
        let turn_index = state
            .tenant_manager
            .send_message(&tenant_id.0, &id, content)
            .await?;

        let body = serde_json::json!({
            "turn_index": turn_index,
        });
        Ok((StatusCode::OK, Json(body)))
    }
}

pub async fn interrupt(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    state.tenant_manager.interrupt(&tenant_id.0, &id).await?;

    Ok(StatusCode::NO_CONTENT)
}
