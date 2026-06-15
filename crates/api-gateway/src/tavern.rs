//! Tavern workflow orchestration integration.
//!
//! State, routes, and handlers for agent/workflow/execution APIs.
//! Merged into the main api-gateway router at startup.

use std::sync::Arc;
use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

// ── State ──

pub struct TavernState {
    pub hero: Arc<tavern_comp::TavernHero>,
    pub registry: Arc<RwLock<tavern_comp::WorkflowRegistry>>,
    pub event_store: Arc<dyn tavern_comp::EventStore>,
    pub tool_registry: Arc<tavern_core::ToolRegistry>,
}

// ── API Types ──

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
    message: String,
    #[serde(skip)]
    status: StatusCode,
}

impl ApiError {
    fn new(status: StatusCode, error: &str, message: &str) -> Self {
        Self { status, error: error.into(), message: message.into() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self)).into_response()
    }
}

#[derive(Deserialize)]
pub struct ExecuteRequest {
    pub task: String,
    #[serde(default)]
    pub context: Value,
}

#[derive(Deserialize)]
pub struct RunWorkflowRequest {
    pub inputs: Value,
    #[serde(default)]
    pub async_mode: bool,
}

// ── Handlers ──

pub async fn health(Extension(state): Extension<Arc<TavernState>>) -> impl IntoResponse {
    let agent_count = state.hero.list_agents_summary().await.len();
    let workflow_count = state.registry.read().await.list_all().len();
    Json(json!({
        "status": "ok", "component": "tavern",
        "agents": agent_count, "workflows": workflow_count,
    }))
}

pub async fn list_agents(Extension(state): Extension<Arc<TavernState>>) -> impl IntoResponse {
    Json(state.hero.list_agents_summary().await)
}

pub async fn get_agent(
    Extension(state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.hero.get_agent(&id).await {
        Some(agent) => Json(serde_json::to_value(agent).unwrap_or_default()).into_response(),
        None => ApiError::new(StatusCode::NOT_FOUND, "AgentNotFound", &format!("Agent '{}' not found", id)).into_response(),
    }
}

pub async fn execute_agent(
    Extension(state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
    Json(req): Json<ExecuteRequest>,
) -> impl IntoResponse {
    match state.hero.execute(&id, &req.task, Some(req.context)).await {
        Ok(result) => Json(json!({"result": result})).into_response(),
        Err(e) => map_hero_error(e),
    }
}

pub async fn list_workflows(Extension(state): Extension<Arc<TavernState>>) -> impl IntoResponse {
    let registry = state.registry.read().await;
    Json(registry.list_all())
}

pub async fn get_workflow(
    Extension(state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let registry = state.registry.read().await;
    match registry.get(&id) {
        Some(wf) => Json(serde_json::to_value(wf).unwrap_or_default()).into_response(),
        None => ApiError::new(StatusCode::NOT_FOUND, "WorkflowNotFound", &format!("Workflow '{}' not found", id)).into_response(),
    }
}

pub async fn run_workflow(
    Extension(state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
    Json(req): Json<RunWorkflowRequest>,
) -> impl IntoResponse {
    let workflow = {
        let registry = state.registry.read().await;
        match registry.get(&id).cloned() {
            Some(w) => w,
            None => return ApiError::new(StatusCode::NOT_FOUND, "WorkflowNotFound", &format!("Workflow '{}' not found", id)).into_response(),
        }
    };
    let engine = tavern_comp::WorkflowEngine::new(state.hero.clone())
        .with_store(state.event_store.clone());
    match engine.run(&workflow, req.inputs).await {
        Ok(result) => Json(serde_json::to_value(result).unwrap_or_default()).into_response(),
        Err(e) => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "WorkflowError", &e.to_string()).into_response(),
    }
}

pub async fn start_workflow(
    Extension(state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
    Json(req): Json<RunWorkflowRequest>,
) -> impl IntoResponse {
    let workflow = {
        let registry = state.registry.read().await;
        match registry.get(&id).cloned() {
            Some(w) => w,
            None => return ApiError::new(StatusCode::NOT_FOUND, "WorkflowNotFound", &format!("Workflow '{}' not found", id)).into_response(),
        }
    };
    let engine = tavern_comp::WorkflowEngine::new(state.hero.clone())
        .with_store(state.event_store.clone());
    match engine.start(&workflow, req.inputs).await {
        Ok(handle) => Json(json!({"execution_id": handle.id})).into_response(),
        Err(e) => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "WorkflowError", &e.to_string()).into_response(),
    }
}

pub async fn get_execution(
    Extension(state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.event_store.read_stream(&id).await {
        Ok(events) => Json(json!({"events": events})).into_response(),
        Err(e) => ApiError::new(StatusCode::NOT_FOUND, "ExecutionNotFound", &e.to_string()).into_response(),
    }
}

// ── Routes ──

pub fn routes() -> axum::Router<()> {
    use axum::routing::{delete, get, post};

    axum::Router::new()
        .route("/health", get(health))
        .route("/agents", get(list_agents))
        .route("/agents/{id}", get(get_agent))
        .route("/agents/{id}/execute", post(execute_agent))
        .route("/workflows", get(list_workflows))
        .route("/workflows/{id}", get(get_workflow))
        .route("/workflows/{id}/run", post(run_workflow))
        .route("/workflows/{id}/start", post(start_workflow))
        .route("/executions/{id}", get(get_execution))
}

// ── Helpers ──

fn map_hero_error(e: tavern_comp::TavernError) -> axum::response::Response {
    match &e {
        tavern_comp::TavernError::AgentNotFound { id } => {
            ApiError::new(StatusCode::NOT_FOUND, "AgentNotFound", &format!("Agent '{}' not found", id)).into_response()
        }
        _ => {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "InternalError", &e.to_string()).into_response()
        }
    }
}
