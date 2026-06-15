//! Minimal Tavern integration — health check proof.

use std::sync::Arc;
use axum::{Extension, Json, response::IntoResponse};
use serde_json::json;

pub struct TavernState {
    pub hero: Arc<tavern_comp::TavernHero>,
    pub registry: Arc<tokio::sync::RwLock<tavern_comp::WorkflowRegistry>>,
    pub event_store: Arc<dyn tavern_comp::EventStore>,
}

pub async fn tavern_health(Extension(state): Extension<Arc<TavernState>>) -> impl IntoResponse {
    let agent_count = state.hero.list_agents_summary().await.len();
    let workflow_count = state.registry.read().await.list_all().len();

    Json(json!({
        "status": "ok",
        "component": "tavern",
        "agents": agent_count,
        "workflows": workflow_count,
    }))
}
