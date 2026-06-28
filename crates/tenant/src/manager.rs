use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use agent_core::{HarnessConfig, SessionActor, SessionBuilder};

use crate::error::TenantError;
use crate::events::SessionEventBridge;
use crate::registry::TenantRegistry;
use crate::session_entry::ActiveSession;

/// Webhook configuration for session event delivery.
#[derive(Debug, Clone)]
pub struct WebhookConfig {
    /// Webhook receiver endpoint.
    pub url: String,
    /// Subscribed event types; empty defaults to ["turn_end", "error"].
    pub events: Vec<String>,
    /// HMAC signing secret (optional).
    pub secret: Option<String>,
}

/// Parameters for creating a new session.
#[derive(Debug, Clone)]
pub struct CreateSessionParams {
    pub title: Option<String>,
    pub system_prompt: Option<String>,
    /// External HTTP proxy tools to register for this session.
    pub tools: Vec<agent_core::ToolConfig>,
    /// Optional webhook configuration for event delivery.
    pub webhook: Option<WebhookConfig>,
    /// Enable Pawbun built-in tools (default true).
    pub builtin_tools_enabled: bool,
    /// Pawbun tool names to exclude.
    pub builtin_tools_disabled: Vec<String>,
    /// Execution strategy for this session.
    pub strategy: agent_core::SessionStrategy,
}

impl Default for CreateSessionParams {
    fn default() -> Self {
        Self {
            title: None,
            system_prompt: None,
            tools: Vec::new(),
            webhook: None,
            builtin_tools_enabled: true,
            builtin_tools_disabled: Vec::new(),
            strategy: agent_core::SessionStrategy::default(),
        }
    }
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
    async fn interrupt(&self, tenant_id: &str, session_id: &Uuid) -> Result<(), TenantError>;

    /// Subscribe to AgentEvent stream for a session.
    /// Drop the receiver to cancel subscription.
    async fn subscribe_events(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<tokio::sync::mpsc::Receiver<agent_core::AgentEvent>, TenantError>;

    /// Delete a session and release all associated resources.
    async fn delete_session(&self, tenant_id: &str, session_id: &Uuid) -> Result<(), TenantError>;

    /// Update session metadata (partial update).
    async fn update_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        updates: SessionUpdates,
    ) -> Result<SessionInfo, TenantError>;

    /// Trigger manual compaction for a session.
    async fn compact_session(&self, tenant_id: &str, session_id: &Uuid) -> Result<(), TenantError>;

    /// Get full message history for a session.
    async fn get_session_messages(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<Vec<agent_core::types::AgentMessage>, TenantError>;

    /// Get the current state of a session.
    async fn get_session_state(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<(agent_core::SessionState, Option<String>), TenantError>;

    /// Get quota information for a tenant.
    async fn get_quota(&self, tenant_id: &str) -> Result<QuotaInfo, TenantError>;

    /// Return the total number of active sessions across all tenants.
    fn active_session_count(&self) -> usize;

    /// Create multiple sessions from a shared template.
    async fn batch_create_sessions(
        &self,
        tenant_id: &str,
        count: usize,
        template: CreateSessionParams,
    ) -> Result<BatchCreateResult, TenantError>;

    /// Clone an existing session (copy config, not history).
    async fn clone_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        title: Option<String>,
    ) -> Result<SessionInfo, TenantError>;

    /// Reset a session (clear history, keep config).
    async fn reset_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<agent_core::SessionState, TenantError>;

    /// Send a message and wait for the turn to complete.
    async fn send_message_and_wait(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        content: Vec<ai_provider::Content>,
        timeout_ms: u64,
    ) -> Result<WaitResult, TenantError>;

    /// Gracefully shut down all sessions.
    async fn shutdown(&self);

    /// Mark a session as completed normally (non-error termination).
    /// Default no-op for implementations that don't track lifecycle.
    async fn complete_session(&self, _tenant_id: &str, _session_id: &Uuid) -> Result<(), TenantError> {
        Ok(())
    }

    /// Returns per-tenant active session counts keyed by tenant_id.
    async fn active_session_counts(&self) -> Result<std::collections::HashMap<String, usize>, TenantError> {
        let mut m = std::collections::HashMap::new();
        m.insert("__total__".into(), self.active_session_count());
        Ok(m)
    }
}

/// Quota information for a tenant.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QuotaInfo {
    pub tenant_id: String,
    pub max_concurrent_sessions: usize,
    pub active_sessions: usize,
    pub max_tokens_per_day: u64,
    pub tokens_used_today: u64,
    pub max_tool_calls_per_minute: u64,
    pub tool_calls_in_last_minute: u64,
    pub default_model: String,
    pub available_models: Vec<String>,
}

/// Result of a batch session creation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchCreateResult {
    pub created: Vec<SessionInfo>,
    pub failed: Vec<BatchFailure>,
}

/// A single failure in a batch create operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchFailure {
    pub reason: String,
}

/// Result of a synchronous wait operation.
#[derive(Debug, Clone)]
pub enum WaitResult {
    Completed {
        turn_index: u64,
        messages: Vec<agent_core::types::AgentMessage>,
    },
    Timeout {
        turn_index: u64,
    },
}

/// Default implementation of `TenantManager`.
///
/// Manages a `TenantRegistry`, creates `SessionActor` instances per session,
/// and bridges `AgentEvent` streams to subscribers.
pub struct TenantManagerImpl {
    registry: Arc<TenantRegistry>,
    runtime_config: Arc<HarnessConfig>,

    sessions: DashMap<(String, Uuid), ActiveSession>,
    /// Maximum allowed synchronous wait timeout in milliseconds.
    max_sync_wait_ms: u64,
    /// Optional metrics registry for per-tenant observability.
    metrics: Option<Arc<observability::MetricsRegistry>>,
}

impl TenantManagerImpl {
    /// Create a new `TenantManagerImpl`.
    pub fn new(
        registry: Arc<TenantRegistry>,
        runtime_config: Arc<HarnessConfig>,
        metrics: Option<Arc<observability::MetricsRegistry>>,
    ) -> Self {
        // Spawn background session cleanup task
        if let Some(ref store) = runtime_config.store {
            let store = store.clone();
            let retention_days = runtime_config.session_retention_days;
            let interval_hours = runtime_config.session_cleanup_interval_hours;

            let retention = std::time::Duration::from_secs(retention_days as u64 * 86400);
            let interval = std::time::Duration::from_secs(interval_hours as u64 * 3600);

            tokio::spawn(async move {
                // Wait for the first interval before starting
                tokio::time::sleep(interval).await;

                loop {
                    match store.cleanup_expired_sessions(retention).await {
                        Ok(0) => {
                            tracing::debug!("session cleanup: no expired sessions found");
                        }
                        Ok(count) => {
                            tracing::info!(count, "session cleanup: deleted expired sessions");
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "session cleanup failed");
                        }
                    }
                    tokio::time::sleep(interval).await;
                }
            });
        }

        Self {
            registry,
            runtime_config,
            sessions: DashMap::new(),
            max_sync_wait_ms: 60_000,
            metrics,
        }
    }

    /// Set the maximum synchronous wait timeout in milliseconds.
    pub fn with_max_sync_wait_ms(mut self, ms: u64) -> Self {
        self.max_sync_wait_ms = ms;
        self
    }

    /// Validate external tool endpoints for SSRF and URL format.
    fn validate_external_tool_endpoints(
        &self,
        tools: &[agent_core::ToolConfig],
    ) -> Result<(), TenantError> {
        for tool_config in tools {
            if url::Url::parse(&tool_config.endpoint).is_err() {
                return Err(TenantError::BadRequest(format!(
                    "tool_endpoint_invalid: {}",
                    tool_config.endpoint
                )));
            }
            if self
                .runtime_config
                .ssrf_policy
                .is_internal_endpoint(&tool_config.endpoint)
            {
                return Err(TenantError::BadRequest(format!(
                    "tool_endpoint_forbidden: {}",
                    tool_config.endpoint
                )));
            }
        }
        Ok(())
    }

    /// Register a webhook event listener on the session actor if configured.
    fn setup_webhook(
        &self,
        actor: &mut SessionActor,
        webhook_config: &Option<WebhookConfig>,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<(), TenantError> {
        let Some(webhook) = webhook_config else {
            return Ok(());
        };
        if url::Url::parse(&webhook.url).is_err() {
            return Err(TenantError::BadRequest(format!(
                "webhook_url_invalid: {}",
                webhook.url
            )));
        }
        if self
            .runtime_config
            .ssrf_policy
            .is_internal_endpoint(&webhook.url)
        {
            return Err(TenantError::BadRequest(format!(
                "webhook_url_forbidden: {}",
                webhook.url
            )));
        }
        let listener = crate::events::WebhookEventListener::new(
            webhook.clone(),
            tenant_id.to_string(),
            session_id.to_string(),
            self.runtime_config.http_client.clone(),
        );
        actor.add_event_listener(Arc::new(listener));
        Ok(())
    }

    /// Finalize session setup: event bridge, abort token, and registration.
    #[allow(clippy::too_many_arguments)]
    fn insert_active_session(
        &self,
        tenant_id: &str,
        session_id: Uuid,
        mut actor: SessionActor,
        guard: crate::supervisor::SessionGuard,
        system_prompt: String,
        params: CreateSessionParams,
        tools: Vec<agent_core::AgentToolRef>,
    ) -> SessionInfo {
        let (event_tx, _) = tokio::sync::broadcast::channel(256);
        let bridge = Arc::new(SessionEventBridge::new(event_tx));
        actor.add_event_listener(bridge.clone());
        let abort_token = Arc::new(std::sync::Mutex::new(actor.abort_token()));

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
            model: self.runtime_config.default_model.clone(),
        };

        self.sessions.insert(
            (tenant_id.to_string(), session_id),
            ActiveSession {
                actor: Arc::new(Mutex::new(actor)),
                abort_token,
                _guard: guard,
                info: info.clone(),
                bridge,
                turn_counter: AtomicU64::new(0),
                tools,
                webhook: params.webhook.clone(),
                original_tools: params.tools.clone(),
                builtin_tools_enabled: params.builtin_tools_enabled,
                builtin_tools_disabled: params.builtin_tools_disabled.clone(),
                strategy: params.strategy.clone(),
            },
        );

        info
    }
}

/// Heuristic token estimation: ~4 chars per token for most LLMs.
fn estimate_input_tokens(content: &[ai_provider::Content]) -> u64 {
    let chars: usize = content
        .iter()
        .map(|c| match c {
            ai_provider::Content::Text { text, .. } => text.len(),
            ai_provider::Content::Image { .. } => 1024,
            ai_provider::Content::Audio { .. } => 1024,
            ai_provider::Content::Video { .. } => 2048,
            _ => 0,
        })
        .sum();
    (chars / 4).max(1) as u64
}

#[async_trait]
impl TenantManager for TenantManagerImpl {
    async fn create_session(
        &self,
        tenant_id: &str,
        params: CreateSessionParams,
    ) -> Result<SessionInfo, TenantError> {
        let guard = self
            .registry
            .get(tenant_id)
            .ok_or_else(|| TenantError::TenantNotFound(tenant_id.to_string()))?
            .reserve_session()?;
        let session_id = Uuid::new_v4();

        self.validate_external_tool_endpoints(&params.tools)?;

        let system_prompt = params
            .system_prompt
            .clone()
            .unwrap_or_else(|| self.runtime_config.default_system_prompt.clone());
        let mut built = SessionBuilder::new(&self.runtime_config)
            .tenant_id(tenant_id)
            .session_id(session_id.to_string())
            .system_prompt(system_prompt.clone())
            .model(self.runtime_config.default_model.clone())
            .with_external_tools(params.tools.clone())
            .with_builtin_tools_config(
                params.builtin_tools_enabled,
                params.builtin_tools_disabled.clone(),
            )
            .with_strategy(params.strategy.clone())
            .build()
            .await
            .map_err(|e| TenantError::Internal {
                tenant_id: tenant_id.to_string(),
                message: format!("session build failed: {}", e),
            })?;

        self.setup_webhook(
            &mut built.actor,
            &params.webhook,
            tenant_id,
            &session_id.to_string(),
        )?;

        let info = self.insert_active_session(
            tenant_id,
            session_id,
            built.actor,
            guard,
            system_prompt,
            params,
            built.tools,
        );

        info!(
            tenant_id = %tenant_id,
            session_id = %session_id,
            "session created"
        );

        if let Some(ref m) = self.metrics {
            m.increment_counter(
                "pandaria_sessions_total",
                &[("tenant_id", tenant_id), ("status", "created")],
                1,
            );
        }

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
            .ok_or_else(|| TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id)))?;

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
            .ok_or_else(|| TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id)))?;

        // Hard quota check before invoking the provider.
        let supervisor = self
            .registry
            .get(tenant_id)
            .ok_or_else(|| TenantError::TenantNotFound(tenant_id.to_string()))?;
        let estimated_input = estimate_input_tokens(&content);
        supervisor.check_quota(crate::tenant::QuotaCheck::TokenUsage {
            input: estimated_input,
            output: 0,
        })?;
        supervisor.check_quota(crate::tenant::QuotaCheck::CpuBudget)?;

        let turn_index = entry.turn_counter.fetch_add(1, Ordering::SeqCst);

        {
            let mut actor = entry.actor.lock().await;
            actor
                .prompt_with_content(content)
                .await
                .map_err(|e| map_agent_error(e, tenant_id))?;
        }

        Ok(turn_index)
    }

    async fn interrupt(&self, tenant_id: &str, session_id: &Uuid) -> Result<(), TenantError> {
        let entry = self
            .sessions
            .get(&(tenant_id.to_string(), *session_id))
            .ok_or_else(|| TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id)))?;

        // Cancel the token without holding the actor lock — this allows
        // abort to reach the in-flight LLM stream even while prompt()
        // is running.
        entry
            .abort_token
            .lock()
            .expect("abort_token lock poisoned")
            .cancel();

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
            .ok_or_else(|| TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id)))?;

        Ok(entry.bridge.subscribe())
    }

    async fn delete_session(&self, tenant_id: &str, session_id: &Uuid) -> Result<(), TenantError> {
        let key = (tenant_id.to_string(), *session_id);
        let entry = self
            .sessions
            .remove(&key)
            .ok_or_else(|| TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id)))?
            .1;

        // Immediately drop the session guard to release the slot BEFORE
        // any async operations. This ensures concurrent requests can
        // acquire the slot without waiting for graceful shutdown.
        let (actor, abort_token, _guard, _info, _bridge, _turn_counter) = (
            entry.actor,
            entry.abort_token,
            entry._guard,
            entry.info,
            entry.bridge,
            entry.turn_counter,
        );
        drop(_guard); // slot released here

        // Gracefully shut down the actor so buffered events are drained
        // (e.g. webhook deliveries) before the session is dropped.
        {
            let mut actor = actor.lock().await;
            actor.shutdown().await;
        }

        // Cancel any in-flight operation.
        abort_token
            .lock()
            .expect("abort_token lock poisoned")
            .cancel();

        // Notify external memory system of session deletion (optional).
        if let Some(ref mem) = self.runtime_config.memory_store {
            let mem_ctx = agent_core::memory::MemoryContext {
                tenant_id: tenant_id.to_string(),
                session_id: session_id.to_string(),
                user_id: None,
                model: String::new(),
                session_started_at: std::time::SystemTime::now(),
            };
            if let Err(e) = mem.forget_session(&mem_ctx).await {
                warn!(
                    tenant_id = %tenant_id,
                    session_id = %session_id,
                    error = %e,
                    "memory: forget_session failed"
                );
            }
        }

        info!(
            tenant_id = %tenant_id,
            session_id = %session_id,
            "session deleted"
        );

        if let Some(ref m) = self.metrics {
            m.increment_counter(
                "pandaria_sessions_total",
                &[("tenant_id", tenant_id), ("status", "failed")],
                1,
            );
        }

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
            .ok_or_else(|| TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id)))?;

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

    async fn compact_session(&self, tenant_id: &str, session_id: &Uuid) -> Result<(), TenantError> {
        let entry = self
            .sessions
            .get(&(tenant_id.to_string(), *session_id))
            .ok_or_else(|| TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id)))?;

        let mut actor = entry.actor.lock().await;
        actor
            .compact(None)
            .await
            .map_err(|e| map_agent_error(e, tenant_id))?;

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
            .ok_or_else(|| TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id)))?;

        let actor = entry.actor.lock().await;
        Ok(actor.messages())
    }

    async fn get_session_state(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<(agent_core::SessionState, Option<String>), TenantError> {
        let entry = self
            .sessions
            .get(&(tenant_id.to_string(), *session_id))
            .ok_or_else(|| TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id)))?;

        let actor = entry.actor.lock().await;
        Ok((actor.state(), actor.error_reason()))
    }

    async fn get_quota(&self, tenant_id: &str) -> Result<QuotaInfo, TenantError> {
        let supervisor = self
            .registry
            .get(tenant_id)
            .ok_or_else(|| TenantError::TenantNotFound(tenant_id.to_string()))?;

        let status = supervisor.quota_status();
        let quota = supervisor.quota();
        Ok(QuotaInfo {
            tenant_id: tenant_id.to_string(),
            max_concurrent_sessions: quota.max_concurrent_sessions as usize,
            active_sessions: status.active_sessions as usize,
            max_tokens_per_day: quota.max_tokens_per_day,
            tokens_used_today: status.tokens_consumed,
            max_tool_calls_per_minute: quota.max_tool_calls_per_minute as u64,
            tool_calls_in_last_minute: status.tool_calls_in_window as u64,
            default_model: self.runtime_config.default_model.clone(),
            available_models: self.runtime_config.available_models.clone(),
        })
    }

    async fn batch_create_sessions(
        &self,
        tenant_id: &str,
        count: usize,
        template: CreateSessionParams,
    ) -> Result<BatchCreateResult, TenantError> {
        let max_count = 10usize;
        if count > max_count {
            return Err(TenantError::BadRequest(format!(
                "batch_size_exceeded: max {}",
                max_count
            )));
        }

        let supervisor = self
            .registry
            .get(tenant_id)
            .ok_or_else(|| TenantError::TenantNotFound(tenant_id.to_string()))?;

        let current = supervisor.active_session_count();
        let max = supervisor.max_concurrent_sessions();
        if current + count > max {
            return Err(TenantError::SessionLimitExceeded {
                tenant_id: tenant_id.to_string(),
                max: max as u32,
                current: current as u32,
            });
        }

        let mut created = vec![];
        for _ in 0..count {
            match self.create_session(tenant_id, template.clone()).await {
                Ok(info) => created.push(info),
                Err(e) => {
                    for info in &created {
                        let _ = self.delete_session(tenant_id, &info.id).await;
                    }
                    return Err(TenantError::Internal {
                        tenant_id: tenant_id.to_string(),
                        message: format!("batch_create_failed: {}", e),
                    });
                }
            }
        }

        Ok(BatchCreateResult {
            created,
            failed: vec![],
        })
    }

    async fn clone_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        title: Option<String>,
    ) -> Result<SessionInfo, TenantError> {
        let entry = self
            .sessions
            .get(&(tenant_id.to_string(), *session_id))
            .ok_or_else(|| TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id)))?;

        let template = CreateSessionParams {
            title,
            system_prompt: Some({
                let actor = entry.actor.lock().await;
                actor.system_prompt()
            }),
            tools: entry.original_tools.clone(),
            webhook: entry.webhook.clone(),
            builtin_tools_enabled: entry.builtin_tools_enabled,
            builtin_tools_disabled: entry.builtin_tools_disabled.clone(),
            strategy: entry.strategy.clone(),
        };

        self.create_session(tenant_id, template).await
    }

    async fn reset_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<agent_core::SessionState, TenantError> {
        let entry = self
            .sessions
            .get(&(tenant_id.to_string(), *session_id))
            .ok_or_else(|| TenantError::SessionNotFound(format!("{}:{}", tenant_id, session_id)))?;

        let mut actor = entry.actor.lock().await;
        let new_token = actor
            .reset()
            .await
            .map_err(|e| map_agent_error(e, tenant_id))?;
        *entry.abort_token.lock().expect("abort_token lock poisoned") = new_token;
        Ok(actor.state())
    }

    async fn send_message_and_wait(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        content: Vec<ai_provider::Content>,
        timeout_ms: u64,
    ) -> Result<WaitResult, TenantError> {
        let mut rx = self.subscribe_events(tenant_id, session_id).await?;
        let turn_index = self.send_message(tenant_id, session_id, content).await?;

        let timeout = std::time::Duration::from_millis(timeout_ms.min(self.max_sync_wait_ms));
        let result = tokio::time::timeout(timeout, async {
            while let Some(event) = rx.recv().await {
                match event {
                    agent_core::AgentEvent::TurnEnd { messages, .. } => {
                        return Ok(WaitResult::Completed {
                            turn_index,
                            messages,
                        });
                    }
                    agent_core::AgentEvent::Error { error } => {
                        return Err(map_agent_error(error, tenant_id));
                    }
                    _ => {}
                }
            }
            Ok(WaitResult::Timeout { turn_index })
        })
        .await;

        match result {
            Ok(Ok(wait_result)) => Ok(wait_result),
            Ok(Err(e)) => Err(e),
            Err(_) => Ok(WaitResult::Timeout { turn_index }),
        }
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

    fn active_session_count(&self) -> usize {
        self.sessions.len()
    }

    async fn complete_session(&self, tenant_id: &str, _session_id: &Uuid) -> Result<(), TenantError> {
        if let Some(ref m) = self.metrics {
            m.increment_counter(
                "pandaria_sessions_total",
                &[("tenant_id", tenant_id), ("status", "completed")],
                1,
            );
        }
        Ok(())
    }

    async fn active_session_counts(&self) -> Result<std::collections::HashMap<String, usize>, TenantError> {
        Ok(self.registry.active_session_counts())
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
        AgentError::SessionInError { reason } => TenantError::SessionInError(reason.clone()),
        _ => TenantError::Internal {
            tenant_id: tenant_id.into(),
            message: format!("agent error: {}", e),
        },
    }
}
