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
        StatusCode::ACCEPTED,
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
