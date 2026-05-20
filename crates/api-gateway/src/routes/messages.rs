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
    types::{SendMessageRequest, SendMessageResponse},
};
use crate::server::AppState;

/// 发送消息并阻塞直到当前 turn 完成。
/// 客户端应在调用此端点**之前**先订阅 `/events` SSE，否则可能错过事件。
pub async fn send(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
    Json(req): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<SendMessageResponse>), GatewayError> {
    let turn_index = state
        .tenant_manager
        .send_message(&tenant_id.0, &id, req.content)
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
