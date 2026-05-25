use std::sync::Arc;

use axum::response::IntoResponse;

use crate::server::AppState;

pub async fn get(state: axum::extract::State<Arc<AppState>>) -> impl IntoResponse {
    let active_sessions = state.tenant_manager.active_session_count();

    let body = format!(
        "# HELP pandaria_active_sessions Number of active sessions\n\
         # TYPE pandaria_active_sessions gauge\n\
         pandaria_active_sessions {}\n",
        active_sessions
    );

    ([("content-type", "text/plain; charset=utf-8")], body)
}
