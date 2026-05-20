use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    error::GatewayError,
    middleware::TenantId,
    types::{MessageContentPart, SendMessageRequest, SendMessageResponse},
};
use crate::server::AppState;

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
    Json(req): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<SendMessageResponse>), GatewayError> {
    let turn_index = state
        .tenant_manager
        .send_message(&tenant_id.0, &id, convert_content(req.content))
        .await?;

    Ok((
        StatusCode::OK,
        Json(SendMessageResponse { turn_index }),
    ))
}

pub async fn interrupt(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    state
        .tenant_manager
        .interrupt(&tenant_id.0, &id)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
