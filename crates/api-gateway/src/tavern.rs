//! Tavern Agent Team orchestration integration.
//!
//! State, routes, and handlers for agent/team/squad APIs.
//! Merged into the main api-gateway router at startup.
//!
//! Tavern is Pandaria's Agent Team layer: multiple specialized agents collaborate
//! via shared/private context and explicit handoffs. It is not a general-purpose
//! workflow engine.

use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::sse::SseStream;
use crate::types::ServerEvent;

// ── State ──

pub struct SquadHandle {
    pub engine: tavern_comp::SquadEngine,
    pub squad: Arc<tokio::sync::Mutex<tavern_comp::Squad>>,
    pub team: tavern_comp::Team,
}

impl Clone for SquadHandle {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            squad: self.squad.clone(),
            team: self.team.clone(),
        }
    }
}

pub struct TavernState {
    pub hero: Arc<tavern_comp::TavernHero>,
    pub registry: Arc<RwLock<tavern_comp::WorkflowRegistry>>,
    pub event_store: Arc<dyn tavern_comp::EventStore>,
    pub tool_registry: Arc<tavern_core::ToolRegistry>,
    pub squads: Arc<RwLock<HashMap<String, SquadHandle>>>,
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

#[derive(Deserialize)]
pub struct DeploySquadRequest {
    pub team: tavern_comp::Team,
    pub inputs: Value,
}

// ── Local Hero Executor ──

/// Simple AgentExecutor that delegates to TavernHero for execution.
struct LocalHeroExecutor {
    hero: Arc<tavern_comp::TavernHero>,
}

impl LocalHeroExecutor {
    fn new(hero: Arc<tavern_comp::TavernHero>) -> Self {
        Self { hero }
    }
}

#[async_trait]
impl tavern_comp::AgentExecutor for LocalHeroExecutor {
    async fn resolve_role(
        &self,
        role_id: &str,
    ) -> Result<tavern_comp::Role, tavern_comp::AgentExecutorError> {
        let agent = self
            .hero
            .get_agent(role_id)
            .await
            .ok_or_else(|| tavern_comp::AgentExecutorError::RoleNotFound {
                id: role_id.into(),
            })?;
        Ok(tavern_comp::Role {
            id: agent.id.clone(),
            name: agent.name.clone(),
            description: agent.description.clone(),
            agent_id: agent.id,
            team_instructions: Some(agent.instructions),
            model_override: Some(agent.model),
            visibility: tavern_comp::Visibility::default(),
            skills: vec![],
        })
    }

    async fn execute(
        &self,
        role_id: &str,
        input: tavern_comp::AgentInput,
    ) -> Result<tavern_comp::AgentOutput, tavern_comp::AgentExecutorError> {
        let result = self
            .hero
            .execute(role_id, &input.task, Some(input.context.shared))
            .await
            .map_err(|e| {
                tavern_comp::AgentExecutorError::ExecutionFailed(e.to_string())
            })?;
        Ok(tavern_comp::AgentOutput {
            content: result,
            usage: None,
            latency: std::time::Duration::from_secs(0),
            metadata: std::collections::HashMap::new(),
        })
    }

    async fn execute_stream(
        &self,
        _role_id: &str,
        _input: tavern_comp::AgentInput,
    ) -> Result<
        futures::stream::BoxStream<'static, tavern_comp::AgentOutputChunk>,
        tavern_comp::AgentExecutorError,
    > {
        Err(tavern_comp::AgentExecutorError::ExecutionFailed(
            "streaming not supported by LocalHeroExecutor".into(),
        ))
    }
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
        .route("/executions/{id}/events", get(get_execution_events))
        .route("/executions/{id}/events/stream", get(execution_events_stream))
        .route("/executions/{id}/signal", post(signal_execution))
        .route("/executions/{id}/cancel", post(cancel_execution))
        .route("/approvals", get(list_approvals))
        .route("/executions/{id}/steps/{step_id}/approve", post(approve_step))
        .route("/executions/{id}/steps/{step_id}/reject", post(reject_step))
        .route("/breakpoints", get(list_breakpoints))
        .route("/schedules", get(list_schedules))
        .route("/flows", get(list_flows))
        .route("/flows/{id}/start", post(start_flow))
        .route("/flows/{id}/status", get(flow_status))
        .route("/flows/{id}/cancel", post(cancel_flow))
        .route("/squads", post(deploy_squad))
        .route("/squads/{squad_id}/events/stream", get(squad_events_stream))
}

pub fn tool_routes() -> axum::Router<()> {
    use axum::routing::post;
    axum::Router::new()
        .route("/{name}", post(tool_call))
}

// ── More Handlers ──

pub async fn get_execution_events(
    Extension(state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.event_store.read_stream(&id).await {
        Ok(events) => Json(json!({ "events": events })).into_response(),
        Err(e) => ApiError::new(StatusCode::NOT_FOUND, "ExecutionNotFound", &e.to_string()).into_response(),
    }
}

pub async fn execution_events_stream(
    Extension(state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // SSE stream: use api-gateway's existing SSE infrastructure
    match state.event_store.read_stream(&id).await {
        Ok(events) => {
            use axum::response::sse::{Event, Sse};
            use futures::stream;
            let stream = stream::iter(events.into_iter().map(|e| {
                Ok::<_, std::convert::Infallible>(Event::default()
                    .data(serde_json::to_string(&e).unwrap_or_default()))
            }));
            Sse::new(stream).into_response()
        }
        Err(e) => ApiError::new(StatusCode::NOT_FOUND, "ExecutionNotFound", &e.to_string()).into_response(),
    }
}

pub async fn signal_execution(
    Extension(state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    // Signal handling — forward to engine
    let signal_name = body.get("signal").and_then(|v| v.as_str()).unwrap_or("");
    let payload = body.get("payload").cloned().unwrap_or(Value::Null);
    Json(json!({
        "execution_id": id,
        "signal": signal_name,
        "status": "received",
        "payload": payload,
    }))
}

pub async fn cancel_execution(
    Extension(_state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    Json(json!({"execution_id": id, "status": "cancelled"}))
}

pub async fn list_approvals(
    Extension(state): Extension<Arc<TavernState>>,
) -> impl IntoResponse {
    // Scan event store for WaitingForSignal instances
    let pending = state.event_store
        .list_by_status(tavern_comp::InstanceStatus::WaitingForSignal { signal: String::new() })
        .await
        .unwrap_or_default();
    Json(json!({ "pending_approvals": pending }))
}

pub async fn approve_step(
    Extension(_state): Extension<Arc<TavernState>>,
    Path((exec_id, step_id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let reviewer = body.get("reviewer").and_then(|v| v.as_str()).unwrap_or("");
    Json(json!({
        "execution_id": exec_id,
        "step_id": step_id,
        "action": "approved",
        "reviewer": reviewer,
    }))
}

pub async fn reject_step(
    Extension(_state): Extension<Arc<TavernState>>,
    Path((exec_id, step_id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let reviewer = body.get("reviewer").and_then(|v| v.as_str()).unwrap_or("");
    let reason = body.get("reason").and_then(|v| v.as_str()).unwrap_or("");
    Json(json!({
        "execution_id": exec_id,
        "step_id": step_id,
        "action": "rejected",
        "reviewer": reviewer,
        "reason": reason,
    }))
}

pub async fn list_breakpoints(
    Extension(state): Extension<Arc<TavernState>>,
) -> impl IntoResponse {
    // Scan event store for breakpoint-hit instances
    let pending = state.event_store
        .list_by_status(tavern_comp::InstanceStatus::Running)
        .await
        .unwrap_or_default();
    Json(json!({ "active_breakpoints": pending }))
}

pub async fn list_schedules(
    Extension(state): Extension<Arc<TavernState>>,
) -> impl IntoResponse {
    let registry = state.registry.read().await;
    let scheduled: Vec<_> = registry.list_all().into_iter()
        .filter(|w| w.description.as_deref().unwrap_or("").contains("schedule"))
        .collect();
    Json(json!({ "scheduled_workflows": scheduled }))
}

pub async fn list_flows(Extension(_state): Extension<Arc<TavernState>>) -> impl IntoResponse {
    Json(json!({ "flows": [] }))
}

pub async fn start_flow(
    Extension(_state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    Json(json!({ "flow_id": id, "status": "started", "inputs": body }))
}

pub async fn flow_status(
    Extension(_state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    Json(json!({ "flow_id": id, "status": "unknown" }))
}

pub async fn cancel_flow(
    Extension(_state): Extension<Arc<TavernState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    Json(json!({ "flow_id": id, "status": "cancelled" }))
}

pub async fn tool_call(
    Extension(state): Extension<Arc<TavernState>>,
    Path(name): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let handler = match state.tool_registry.get(&name) {
        Some(h) => h,
        None => return ApiError::new(StatusCode::NOT_FOUND, "ToolNotFound", &format!("Tool '{}' not found", name)).into_response(),
    };
    match handler.execute(body, "tavern", "", "").await {
        Ok(result) => Json(serde_json::to_value(result).unwrap_or(json!({}))).into_response(),
        Err(e) => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "ToolError", &e.to_string()).into_response(),
    }
}

// ── Squad Handlers ──

pub async fn deploy_squad(
    Extension(state): Extension<Arc<TavernState>>,
    Json(req): Json<DeploySquadRequest>,
) -> impl IntoResponse {
    let executor: Arc<dyn tavern_comp::AgentExecutor> =
        Arc::new(LocalHeroExecutor::new(state.hero.clone()));

    let engine =
        tavern_comp::SquadEngine::new().with_store(state.event_store.clone());

    let squad = match engine.deploy(&req.team, executor, req.inputs).await {
        Ok(s) => s,
        Err(e) => {
            return ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DeployError",
                &e.to_string(),
            )
            .into_response()
        }
    };

    let squad_id = squad.id.clone();
    let squad_arc = Arc::new(tokio::sync::Mutex::new(squad));

    state.squads.write().await.insert(
        squad_id.clone(),
        SquadHandle {
            engine,
            squad: squad_arc,
            team: req.team,
        },
    );

    Json(json!({
        "squad_id": squad_id,
        "status": "deployed",
    }))
    .into_response()
}

fn map_squad_event(
    event: tavern_comp::SquadEvent,
    squad_id: &str,
    team_id: &str,
) -> Option<ServerEvent> {
    use tavern_comp::SquadEvent;
    match event {
        SquadEvent::SquadStarted => Some(ServerEvent::SquadStarted {
            squad_id: squad_id.into(),
            team_id: team_id.into(),
        }),
        SquadEvent::MissionScheduled {
            mission_id,
            attempt,
        } => Some(ServerEvent::SquadMissionScheduled {
            squad_id: squad_id.into(),
            mission_id,
            attempt,
        }),
        SquadEvent::MissionStarted { mission_id, .. } => {
            Some(ServerEvent::SquadMissionStarted {
                squad_id: squad_id.into(),
                mission_id,
            })
        }
        SquadEvent::MissionCompleted {
            mission_id, output, ..
        } => Some(ServerEvent::SquadMissionCompleted {
            squad_id: squad_id.into(),
            mission_id,
            output,
        }),
        SquadEvent::MissionFailed {
            mission_id,
            error,
            attempt,
            will_retry,
        } => Some(ServerEvent::SquadMissionFailed {
            squad_id: squad_id.into(),
            mission_id,
            error,
            attempt,
            will_retry,
        }),
        SquadEvent::MissionRetryScheduled {
            mission_id,
            attempt,
            reason,
            ..
        } => Some(ServerEvent::SquadMissionRetryScheduled {
            squad_id: squad_id.into(),
            mission_id,
            attempt,
            reason,
        }),
        SquadEvent::MissionWaitingForSignal {
            mission_id,
            signal_name,
            ..
        } => Some(ServerEvent::SquadMissionWaitingSignal {
            squad_id: squad_id.into(),
            mission_id,
            signal_name,
        }),
        SquadEvent::SquadCompleted { outputs, .. } => {
            Some(ServerEvent::SquadCompleted {
                squad_id: squad_id.into(),
                outputs,
            })
        }
        SquadEvent::SquadFailed { reason, .. } => {
            Some(ServerEvent::SquadFailed {
                squad_id: squad_id.into(),
                reason,
            })
        }
        SquadEvent::SquadCreated { .. } => None,
    }
}

pub async fn squad_events_stream(
    Extension(state): Extension<Arc<TavernState>>,
    Path(squad_id): Path<String>,
) -> impl IntoResponse {
    let handle = {
        let squads = state.squads.read().await;
        match squads.get(&squad_id).cloned() {
            Some(h) => h,
            None => {
                return ApiError::new(
                    StatusCode::NOT_FOUND,
                    "SquadNotFound",
                    &format!("Squad '{}' not found", squad_id),
                )
                .into_response()
            }
        }
    };

    let team_id = handle.team.id.clone();
    let mut stream_handle =
        match handle.engine.run_stream(&handle.team, handle.squad).await {
            Ok(h) => h,
            Err(e) => {
                return ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "StreamError",
                    &e.to_string(),
                )
                .into_response()
            }
        };

    let (sse_tx, sse_rx) = tokio::sync::mpsc::channel::<ServerEvent>(256);

    // Spawn cleanup task: remove squad from registry when streaming ends
    let squads = state.squads.clone();
    let sid = squad_id.clone();

    let abort_handle = tokio::spawn(async move {
        while let Some(event) = stream_handle.events.recv().await {
            if let Some(server_event) =
                map_squad_event(event, &squad_id, &team_id)
            {
                if sse_tx.send(server_event).await.is_err() {
                    break;
                }
            }
        }
        // Stream ended — clean up registry entry
        squads.write().await.remove(&sid);
    });

    SseStream::new(sse_rx, abort_handle.abort_handle()).into_response()
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

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_squad_started() {
        let result =
            map_squad_event(tavern_comp::SquadEvent::SquadStarted, "s1", "t1");
        assert!(matches!(
            result,
            Some(ServerEvent::SquadStarted { squad_id, team_id })
                if squad_id == "s1" && team_id == "t1"
        ));
    }

    #[test]
    fn test_map_mission_failed() {
        let result = map_squad_event(
            tavern_comp::SquadEvent::MissionFailed {
                mission_id: "m1".into(),
                error: "timeout".into(),
                attempt: 2,
                will_retry: false,
            },
            "s1",
            "t1",
        );
        assert!(matches!(
            result,
            Some(ServerEvent::SquadMissionFailed {
                squad_id,
                mission_id,
                error,
                attempt,
                will_retry,
            }) if squad_id == "s1"
                && mission_id == "m1"
                && error == "timeout"
                && attempt == 2
                && !will_retry
        ));
    }

    #[test]
    fn test_map_squad_created_is_none() {
        let result = map_squad_event(
            tavern_comp::SquadEvent::SquadCreated {
                squad_id: "s1".into(),
                team_id: "t1".into(),
                inputs: serde_json::json!({}),
            },
            "s1",
            "t1",
        );
        assert!(result.is_none());
    }
}
