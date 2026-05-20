use axum::extract::State;
use axum::http::StatusCode;
use std::sync::Arc;

use crate::server::AppState;

pub async fn get(State(state): State<Arc<AppState>>) -> StatusCode {
    // Lightweight dependency check: list sessions for a non-existent tenant.
    // Any internal error (other than TenantNotFound) indicates a downstream issue.
    match state.tenant_manager.list_sessions("__health_check__").await {
        Ok(_) | Err(tenant::TenantError::TenantNotFound(_)) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}
