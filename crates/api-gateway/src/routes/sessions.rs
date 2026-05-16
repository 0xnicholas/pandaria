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
    types::{
        CreateSessionRequest, SessionInfo, UpdateSessionRequest,
    },
};
use crate::server::AppState;

pub async fn create(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<SessionInfo>), GatewayError> {
    let params = tenant::CreateSessionParams {
        title: req.title,
        system_prompt: req.system_prompt,
    };

    let info = state
        .tenant_manager
        .create_session(&tenant_id.0, params)
        .await?;

    let mut session_info: SessionInfo = info.into();
    session_info.context_window = Some(state.config.default_context_window);

    Ok((StatusCode::CREATED, Json(session_info)))
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
) -> Result<Json<Vec<SessionInfo>>, GatewayError> {
    let infos = state
        .tenant_manager
        .list_sessions(&tenant_id.0)
        .await?;

    let sessions: Vec<SessionInfo> = infos
        .into_iter()
        .map(|info| {
            let mut s: SessionInfo = info.into();
            s.context_window = Some(state.config.default_context_window);
            s
        })
        .collect();

    Ok(Json(sessions))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
) -> Result<Json<SessionInfo>, GatewayError> {
    let info = state
        .tenant_manager
        .get_session(&tenant_id.0, &id)
        .await?;

    let mut session_info: SessionInfo = info.into();
    session_info.context_window = Some(state.config.default_context_window);

    Ok(Json(session_info))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateSessionRequest>,
) -> Result<Json<SessionInfo>, GatewayError> {
    let updates = tenant::SessionUpdates {
        title: Some(req.title),
        model: req.model,
        system_prompt: req.system_prompt,
    };

    let info = state
        .tenant_manager
        .update_session(&tenant_id.0, &id, updates)
        .await?;

    let mut session_info: SessionInfo = info.into();
    session_info.context_window = Some(state.config.default_context_window);

    Ok(Json(session_info))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    state
        .tenant_manager
        .delete_session(&tenant_id.0, &id)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn compact(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    state
        .tenant_manager
        .compact_session(&tenant_id.0, &id)
        .await?;

    Ok(StatusCode::ACCEPTED)
}

pub async fn messages(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<agent_core::AgentMessage>>, GatewayError> {
    let msgs = state
        .tenant_manager
        .get_session_messages(&tenant_id.0, &id)
        .await?;

    Ok(Json(msgs))
}
