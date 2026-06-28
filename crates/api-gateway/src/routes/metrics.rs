use std::sync::Arc;

use axum::response::IntoResponse;

use crate::server::AppState;

pub async fn get(
    state: axum::extract::State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Some(ref registry) = state.metrics_registry {
        // Populate per-tenant active session gauge from live data
        if let Ok(counts) = state.tenant_manager.active_session_counts().await {
            for (tenant_id, count) in &counts {
                registry.set_gauge(
                    "pandaria_sessions_active",
                    &[("tenant_id", tenant_id)],
                    *count as i64,
                );
            }
        }
        let body = registry.export();
        return ([("content-type", "text/plain; charset=utf-8")], body);
    }

    // Fallback: registry not configured — return legacy bare gauge
    let active = state.tenant_manager.active_session_count();
    let body = format!(
        "# HELP pandaria_active_sessions Active sessions\n\
         # TYPE pandaria_active_sessions gauge\n\
         pandaria_active_sessions {}\n",
        active
    );
    ([("content-type", "text/plain; charset=utf-8")], body)
}
