use axum::{
    Json,
    extract::{Extension, Path, State},
    http::StatusCode,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::server::AppState;
use crate::{
    error::GatewayError,
    middleware::TenantId,
    types::{
        BatchCreateRequest, BatchCreateResult, CreateSessionRequest, QuotaInfoResponse,
        ResetSessionResponse, SessionInfo, SessionStateResponse, UpdateSessionRequest,
    },
};

pub async fn create(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<SessionInfo>), GatewayError> {
    let params = tenant::CreateSessionParams {
        title: req.title,
        system_prompt: req.system_prompt,
        tools: req
            .tools
            .into_iter()
            .map(|t| agent_core::ToolConfig {
                name: t.name,
                description: t.description,
                parameters: t.parameters,
                endpoint: t.endpoint,
                timeout_ms: t.timeout_ms,
                headers: t.headers,
            })
            .collect(),
        webhook: req.webhook.map(|w| tenant::WebhookConfig {
            url: w.url,
            events: w.events,
            secret: w.secret,
        }),
    };

    let info = state
        .tenant_manager
        .create_session(&tenant_id.0, params)
        .await?;

    Ok((StatusCode::CREATED, Json(state.enrich_session_info(info))))
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
) -> Result<Json<Vec<SessionInfo>>, GatewayError> {
    let infos = state.tenant_manager.list_sessions(&tenant_id.0).await?;

    let sessions: Vec<SessionInfo> = infos
        .into_iter()
        .map(|info| state.enrich_session_info(info))
        .collect();

    Ok(Json(sessions))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
) -> Result<Json<SessionInfo>, GatewayError> {
    let info = state.tenant_manager.get_session(&tenant_id.0, &id).await?;

    Ok(Json(state.enrich_session_info(info)))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateSessionRequest>,
) -> Result<Json<SessionInfo>, GatewayError> {
    let updates = tenant::SessionUpdates {
        title: req.title.map(Some),
        model: req.model,
        system_prompt: req.system_prompt,
    };

    let info = state
        .tenant_manager
        .update_session(&tenant_id.0, &id, updates)
        .await?;

    Ok(Json(state.enrich_session_info(info)))
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

pub async fn get_state(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
) -> Result<Json<SessionStateResponse>, GatewayError> {
    let (session_state, error_reason) = state
        .tenant_manager
        .get_session_state(&tenant_id.0, &id)
        .await?;

    Ok(Json(SessionStateResponse {
        state: format!("{:?}", session_state).to_lowercase(),
        error_reason,
    }))
}

pub async fn get_quota(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
) -> Result<Json<QuotaInfoResponse>, GatewayError> {
    let quota = state.tenant_manager.get_quota(&tenant_id.0).await?;

    Ok(Json(QuotaInfoResponse {
        tenant_id: quota.tenant_id,
        max_concurrent_sessions: quota.max_concurrent_sessions,
        active_sessions: quota.active_sessions,
        max_tokens_per_day: quota.max_tokens_per_day,
        tokens_used_today: quota.tokens_used_today,
        max_tool_calls_per_minute: quota.max_tool_calls_per_minute,
        tool_calls_in_last_minute: quota.tool_calls_in_last_minute,
        default_model: quota.default_model,
        available_models: quota.available_models,
    }))
}

pub async fn batch_create(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Json(req): Json<BatchCreateRequest>,
) -> Result<(StatusCode, Json<BatchCreateResult>), GatewayError> {
    let template = tenant::CreateSessionParams {
        title: req.template.title,
        system_prompt: req.template.system_prompt,
        tools: req
            .template
            .tools
            .into_iter()
            .map(|t| agent_core::ToolConfig {
                name: t.name,
                description: t.description,
                parameters: t.parameters,
                endpoint: t.endpoint,
                timeout_ms: t.timeout_ms,
                headers: t.headers,
            })
            .collect(),
        webhook: req.template.webhook.map(|w| tenant::WebhookConfig {
            url: w.url,
            events: w.events,
            secret: w.secret,
        }),
    };

    let result = state
        .tenant_manager
        .batch_create_sessions(&tenant_id.0, req.count, template)
        .await?;

    let created: Vec<SessionInfo> = result
        .created
        .into_iter()
        .map(|info| state.enrich_session_info(info))
        .collect();

    Ok((
        StatusCode::CREATED,
        Json(BatchCreateResult {
            created,
            failed: result
                .failed
                .into_iter()
                .map(|f| crate::types::BatchFailure { reason: f.reason })
                .collect(),
        }),
    ))
}

pub async fn clone(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<SessionInfo>), GatewayError> {
    let title = body
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let info = state
        .tenant_manager
        .clone_session(&tenant_id.0, &id, title)
        .await?;

    Ok((StatusCode::CREATED, Json(state.enrich_session_info(info))))
}

pub async fn reset(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
) -> Result<Json<ResetSessionResponse>, GatewayError> {
    let session_state = state
        .tenant_manager
        .reset_session(&tenant_id.0, &id)
        .await?;

    Ok(Json(ResetSessionResponse {
        state: format!("{:?}", session_state).to_lowercase(),
    }))
}

pub async fn messages(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<agent_core::AgentMessage>>, GatewayError> {
    let mut msgs = state
        .tenant_manager
        .get_session_messages(&tenant_id.0, &id)
        .await?;
    // Default downgrade for backward compatibility with old TUI clients
    for msg in &mut msgs {
        downgrade_media_in_message(msg);
    }
    Ok(Json(msgs))
}

/// Downgrade Image/Video/Audio in a message to Text placeholders.
/// Only affects the response, does NOT modify SessionStore data.
fn downgrade_media_in_message(msg: &mut agent_core::AgentMessage) {
    let content = match msg {
        agent_core::AgentMessage::User(u) => Some(&mut u.content),
        agent_core::AgentMessage::Assistant(a) => Some(&mut a.content),
        agent_core::AgentMessage::ToolResult(t) => Some(&mut t.content),
    };
    if let Some(content) = content {
        for c in content.iter_mut() {
            let replacement = match c {
                ai_provider::Content::Image { .. } => Some("[图片内容]"),
                ai_provider::Content::Video { .. } => Some("[视频内容]"),
                ai_provider::Content::Audio { .. } => Some("[音频内容]"),
                _ => None,
            };
            if let Some(text) = replacement {
                *c = ai_provider::Content::Text {
                    text: text.to_string(),
                    text_signature: None,
                };
            }
        }
    }
}
