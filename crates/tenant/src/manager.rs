use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use agent_core::{CompactionConfig, DefaultFileOperationExtractor, SessionActor, SessionConfig, SessionStore};
use agent_core::space::AgentSpace;
use agent_core::skills::SkillLoader;

use crate::error::TenantError;
use crate::events::SessionEventBridge;
use crate::registry::TenantRegistry;
use crate::session_entry::ActiveSession;


/// Parameters for creating a new session.
#[derive(Debug, Clone)]
pub struct CreateSessionParams {
    pub title: Option<String>,
    pub system_prompt: Option<String>,
}

/// Partial updates for an existing session.
#[derive(Debug, Clone, Default)]
pub struct SessionUpdates {
    pub title: Option<Option<String>>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
}

/// Metadata about a session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionInfo {
    pub id: Uuid,
    pub tenant_id: String,
    pub created_at: String,
    pub turn_count: u64,
    pub system_prompt: Option<String>,
    pub title: Option<String>,
    pub model: String,
}

/// Dependency-inversion boundary for tenant/session management.
///
/// Implemented by `TenantManagerImpl` in this crate.
/// Consumed by `api-gateway` (and potentially other entry points).
#[async_trait]
pub trait TenantManager: Send + Sync {
    /// Create a new session.
    async fn create_session(
        &self,
        tenant_id: &str,
        params: CreateSessionParams,
    ) -> Result<SessionInfo, TenantError>;

    /// List all sessions for a tenant.
    async fn list_sessions(&self, tenant_id: &str) -> Result<Vec<SessionInfo>, TenantError>;

    /// Get metadata for a single session.
    async fn get_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<SessionInfo, TenantError>;

    /// Send a user message, triggering a new agent turn.
    /// Returns the turn index (for client correlation with SSE events).
    async fn send_message(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        content: Vec<ai_provider::Content>,
    ) -> Result<u64, TenantError>;

    /// Interrupt the current in-flight turn.
    async fn interrupt(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<(), TenantError>;

    /// Subscribe to AgentEvent stream for a session.
    /// Drop the receiver to cancel subscription.
    async fn subscribe_events(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<tokio::sync::mpsc::Receiver<agent_core::AgentEvent>, TenantError>;

    /// Delete a session and release all associated resources.
    async fn delete_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<(), TenantError>;

    /// Update session metadata (partial update).
    async fn update_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        updates: SessionUpdates,
    ) -> Result<SessionInfo, TenantError>;

    /// Trigger manual compaction for a session.
    async fn compact_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<(), TenantError>;

    /// Get full message history for a session.
    async fn get_session_messages(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<Vec<agent_core::types::AgentMessage>, TenantError>;

    /// Gracefully shut down all sessions.
    async fn shutdown(&self);
}

/// Default implementation of `TenantManager`.
///
/// Manages a `TenantRegistry`, creates `SessionActor` instances per session,
/// and bridges `AgentEvent` streams to subscribers.
pub struct TenantManagerImpl {
    registry: Arc<TenantRegistry>,
    provider: Arc<dyn ai_provider::LlmProvider>,
    store: Option<Arc<dyn SessionStore>>,
    default_model: String,
    default_system_prompt: String,
    #[allow(dead_code)]
    default_context_window: usize,
    sessions: DashMap<(String, Uuid), ActiveSession>,
    /// Optional media provider for generate_media tool.
    media_provider: Option<Arc<dyn ai_provider::MediaProvider>>,
    /// Optional media model registry.
    media_registry: Option<Arc<ai_provider::MediaModelRegistry>>,
    /// Optional per-tenant cost tracker.
    cost_tracker: Option<Arc<crate::meter::CostTracker>>,
}

impl TenantManagerImpl {
    /// Create a new `TenantManagerImpl`.
    pub fn new(
        registry: Arc<TenantRegistry>,
        provider: Arc<dyn ai_provider::LlmProvider>,
        store: Option<Arc<dyn SessionStore>>,
        default_model: impl Into<String>,
        default_system_prompt: impl Into<String>,
        default_context_window: usize,
    ) -> Self {
        Self {
            registry,
            provider,
            store,
            default_model: default_model.into(),
            default_system_prompt: default_system_prompt.into(),
            default_context_window,
            sessions: DashMap::new(),
            media_provider: None,
            media_registry: None,
            cost_tracker: None,
        }
    }

    /// Set an optional media provider and registry for the `generate_media` tool.
    pub fn with_media(
        mut self,
        provider: Arc<dyn ai_provider::MediaProvider>,
        registry: Arc<ai_provider::MediaModelRegistry>,
    ) -> Self {
        self.media_provider = Some(provider);
        self.media_registry = Some(registry);
        self
    }

    /// Set an optional cost tracker for media and LLM cost accounting.
    pub fn with_cost_tracker(mut self, tracker: Arc<crate::meter::CostTracker>) -> Self {
        self.cost_tracker = Some(tracker);
        self
    }
}

#[async_trait]
impl TenantManager for TenantManagerImpl {
    async fn create_session(
        &self,
        tenant_id: &str,
        params: CreateSessionParams,
    ) -> Result<SessionInfo, TenantError> {
        // 1. Validate tenant exists
        let supervisor = self
            .registry
            .get(tenant_id)
            .ok_or_else(|| TenantError::TenantNotFound(tenant_id.to_string()))?;

        // 2. Reserve a session slot (RAII guard). Quota check is implicit.
        let guard = supervisor.reserve_session()?;

        // 3. Create per-session hook dispatcher and tools
        let mut dispatcher = agent_core::DefaultHookDispatcher::new();
        if let Some(ref tracker) = self.cost_tracker {
            let tracker = tracker.clone();
            dispatcher.cost_callback = Some(Arc::new(move |tenant_id, cost| {
                tracker.record_media_call(cost);
                tracing::info!(tenant_id, cost, "media cost recorded");
            }));
        }
        let hook_dispatcher = Arc::new(dispatcher) as Arc<dyn agent_core::HookDispatcher>;
        let mut tools: Vec<Arc<dyn agent_core::types::AgentTool>> = vec![];
        if let (Some(media_provider), Some(media_registry)) = (&self.media_provider, &self.media_registry) {
            let media_tool = Arc::new(agent_core::MediaGenerationTool::new(
                media_provider.clone(),
                media_registry.clone(),
                self.default_model.clone(),
                tenant_id,
            ));
            tools.push(media_tool);
        }

        // 4. Create compaction actor
        let compaction_config = CompactionConfig {
            enabled: true,
            reserve_tokens: 4096,
            keep_recent_tokens: 8192,
        };
        let compaction_actor = Arc::new(agent_core::CompactionActor::new(
            compaction_config,
            self.provider.clone(),
            self.default_model.clone(),
            Arc::new(DefaultFileOperationExtractor::default()),
        ));

        // 5. Load skills for this tenant
        let agent_space = AgentSpace::from_env_or_default();
        let user_skills_dir = agent_space.skills_dir().display().to_string();
        let project_skills_dir = agent_space.workspace_for(tenant_id).join("skills");
        let _ = std::fs::create_dir_all(&project_skills_dir);

        let loader = agent_core::skills::FileSystemSkillLoader {
            user_skills_dir,
            project_skills_dir: project_skills_dir.display().to_string(),
            explicit_paths: vec![],
        };
        let load_result = loader.load_skills().await;
        if !load_result.diagnostics.is_empty() {
            for diag in &load_result.diagnostics {
                tracing::warn!(path = %diag.path, kind = ?diag.kind, "skill diagnostic: {}", diag.message);
            }
        }
        let skills = load_result.skills;

        // 6. Create session actor
        let session_id = Uuid::new_v4();
        let system_prompt = params
            .system_prompt
            .unwrap_or_else(|| self.default_system_prompt.clone());

        let mut actor = SessionActor::new(SessionConfig {
            tenant_id: tenant_id.to_string(),
            session_id: session_id.to_string(),
            system_prompt: system_prompt.clone(),
            model: self.default_model.clone(),
            provider: self.provider.clone(),
            hook_dispatcher,
            compaction_actor,
            tools,
            store: self.store.clone(),
            skills,
        });

        // 6. Set up event bridge and abort token
        let (event_tx, _) = tokio::sync::broadcast::channel(256);
        let bridge = Arc::new(SessionEventBridge::new(event_tx));
        actor.add_event_listener(bridge.clone());
        let abort_token = actor.abort_token();

        let info = SessionInfo {
            id: session_id,
            tenant_id: tenant_id.to_string(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .to_string(),
            turn_count: 0,
            system_prompt: Some(system_prompt),
            title: params.title.clone(),
            model: self.default_model.clone(),
        };

        // 7. Store session handle
        self.sessions.insert(
            (tenant_id.to_string(), session_id),
            ActiveSession {
                actor: Arc::new(Mutex::new(actor)),
                abort_token,
                _guard: guard,
                info: info.clone(),
                bridge,
                turn_counter: AtomicU64::new(0),
            },
        );

        info!(
            tenant_id = %tenant_id,
            session_id = %session_id,
            "session created"
        );

        Ok(info)
    }

    async fn list_sessions(&self, tenant_id: &str) -> Result<Vec<SessionInfo>, TenantError> {
        let mut infos = Vec::new();
        for entry in self.sessions.iter() {
            let (key_tenant_id, _) = entry.key();
            if key_tenant_id == tenant_id {
                let mut info = entry.info.clone();
                info.turn_count = entry.turn_counter.load(Ordering::SeqCst);
                infos.push(info);
            }
        }
        Ok(infos)
    }

    async fn get_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<SessionInfo, TenantError> {
        let entry = self
            .sessions
            .get(&(tenant_id.to_string(), *session_id))
            .ok_or_else(|| {
                TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id))
            })?;

        let mut info = entry.info.clone();
        info.turn_count = entry.turn_counter.load(Ordering::SeqCst);
        Ok(info)
    }

    async fn send_message(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        content: Vec<ai_provider::Content>,
    ) -> Result<u64, TenantError> {
        let entry = self
            .sessions
            .get(&(tenant_id.to_string(), *session_id))
            .ok_or_else(|| {
                TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id))
            })?;

        // Hard quota check before invoking the provider.
        let supervisor = self
            .registry
            .get(tenant_id)
            .ok_or_else(|| TenantError::TenantNotFound(tenant_id.to_string()))?;
        supervisor.check_quota(crate::tenant::QuotaCheck::TokenUsage { input: 0, output: 0 })?;

        let turn_index = entry.turn_counter.fetch_add(1, Ordering::SeqCst);

        {
            let mut actor = entry.actor.lock().await;
            actor.prompt_with_content(content).await.map_err(|e| map_agent_error(e, tenant_id))?;
        }

        Ok(turn_index)
    }

    async fn interrupt(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<(), TenantError> {
        let entry = self
            .sessions
            .get(&(tenant_id.to_string(), *session_id))
            .ok_or_else(|| {
                TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id))
            })?;

        // Cancel the token without holding the actor lock — this allows
        // abort to reach the in-flight LLM stream even while prompt()
        // is running.
        entry.abort_token.cancel();

        warn!(
            tenant_id = %tenant_id,
            session_id = %session_id,
            "session interrupted"
        );

        Ok(())
    }

    async fn subscribe_events(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<tokio::sync::mpsc::Receiver<agent_core::AgentEvent>, TenantError> {
        let entry = self
            .sessions
            .get(&(tenant_id.to_string(), *session_id))
            .ok_or_else(|| {
                TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id))
            })?;

        Ok(entry.bridge.subscribe())
    }

    async fn delete_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<(), TenantError> {
        let key = (tenant_id.to_string(), *session_id);
        let entry = self
            .sessions
            .remove(&key)
            .ok_or_else(|| {
                TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id))
            })?
            .1;

        // Cancel any in-flight operation.
        entry.abort_token.cancel();

        // Extension lifecycle is managed by the caller (api-gateway) via factory.
        // The SessionGuard in `entry._guard` will be dropped here,
        // automatically releasing the session slot.

        info!(
            tenant_id = %tenant_id,
            session_id = %session_id,
            "session deleted"
        );

        Ok(())
    }

    async fn update_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        updates: SessionUpdates,
    ) -> Result<SessionInfo, TenantError> {
        let mut entry = self
            .sessions
            .get_mut(&(tenant_id.to_string(), *session_id))
            .ok_or_else(|| {
                TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id))
            })?;

        // Update stored info and session actor
        if let Some(title) = updates.title {
            entry.info.title = title;
        }
        if let Some(model) = updates.model {
            entry.info.model = model.clone();
            let mut actor = entry.actor.lock().await;
            actor.set_model(model);
        }
        if let Some(system_prompt) = updates.system_prompt {
            entry.info.system_prompt = Some(system_prompt.clone());
            let mut actor = entry.actor.lock().await;
            actor.set_system_prompt(system_prompt);
        }

        let mut info = entry.info.clone();
        info.turn_count = entry.turn_counter.load(Ordering::SeqCst);
        Ok(info)
    }

    async fn compact_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<(), TenantError> {
        let entry = self
            .sessions
            .get(&(tenant_id.to_string(), *session_id))
            .ok_or_else(|| {
                TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id))
            })?;

        let mut actor = entry.actor.lock().await;
        actor.compact(None).await.map_err(|e| map_agent_error(e, tenant_id))?;

        info!(
            tenant_id = %tenant_id,
            session_id = %session_id,
            "session compacted"
        );

        Ok(())
    }

    async fn get_session_messages(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<Vec<agent_core::types::AgentMessage>, TenantError> {
        let entry = self
            .sessions
            .get(&(tenant_id.to_string(), *session_id))
            .ok_or_else(|| {
                TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id))
            })?;

        let actor = entry.actor.lock().await;
        Ok(actor.messages())
    }

    async fn shutdown(&self) {
        let keys: Vec<_> = self
            .sessions
            .iter()
            .map(|entry| (entry.key().0.clone(), entry.key().1))
            .collect();

        for (tenant_id, session_id) in keys {
            let _ = self.delete_session(&tenant_id, &session_id).await;
        }
    }
}

/// Map `AgentError` to `TenantError` preserving semantic meaning where possible.
fn map_agent_error(e: agent_core::AgentError, tenant_id: &str) -> TenantError {
    use agent_core::AgentError;
    match &e {
        AgentError::ContextOverflow(_) => TenantError::Internal {
            tenant_id: tenant_id.into(),
            message: format!("context overflow: {}", e),
        },
        AgentError::LlmError(_) => TenantError::Internal {
            tenant_id: tenant_id.into(),
            message: format!("LLM error: {}", e),
        },
        AgentError::LlmResponseError(_) => TenantError::Internal {
            tenant_id: tenant_id.into(),
            message: format!("LLM response error: {}", e),
        },
        AgentError::Cancelled => TenantError::Internal {
            tenant_id: tenant_id.into(),
            message: "session cancelled".to_string(),
        },
        _ => TenantError::Internal {
            tenant_id: tenant_id.into(),
            message: format!("agent error: {}", e),
        },
    }
}
