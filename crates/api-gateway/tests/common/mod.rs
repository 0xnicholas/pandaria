use api_gateway::{AppState, ServerConfig};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use tenant::{CreateSessionParams, SessionInfo, SessionUpdates, TenantError, TenantManager};
use tokio::sync::mpsc;
use uuid::Uuid;

/// Mock TenantManager for integration tests.
pub struct MockTenantManager {
    pub sessions: Mutex<std::collections::HashMap<String, SessionInfo>>,
    pub event_senders:
        Mutex<std::collections::HashMap<String, Vec<mpsc::Sender<agent_core::AgentEvent>>>>,
}

impl MockTenantManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(std::collections::HashMap::new()),
            event_senders: Mutex::new(std::collections::HashMap::new()),
        }
    }
}

#[async_trait]
impl TenantManager for MockTenantManager {
    async fn create_session(
        &self,
        tenant_id: &str,
        params: CreateSessionParams,
    ) -> Result<SessionInfo, TenantError> {
        let id = Uuid::new_v4();
        let info = SessionInfo {
            id,
            tenant_id: tenant_id.into(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .to_string(),
            turn_count: 0,
            system_prompt: params.system_prompt,
            title: params.title,
            model: "claude-sonnet-4".into(),
        };
        self.sessions
            .lock()
            .unwrap()
            .insert(id.to_string(), info.clone());
        Ok(info)
    }

    async fn list_sessions(&self, tenant_id: &str) -> Result<Vec<SessionInfo>, TenantError> {
        let sessions = self.sessions.lock().unwrap();
        Ok(sessions
            .values()
            .filter(|s| s.tenant_id == tenant_id)
            .cloned()
            .collect())
    }

    async fn get_session(
        &self,
        _tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<SessionInfo, TenantError> {
        self.sessions
            .lock()
            .unwrap()
            .get(&session_id.to_string())
            .cloned()
            .ok_or_else(|| TenantError::SessionNotFound(session_id.to_string()))
    }

    async fn send_message(
        &self,
        _tenant_id: &str,
        _session_id: &Uuid,
        _content: Vec<ai_provider::Content>,
    ) -> Result<u64, TenantError> {
        Ok(1)
    }

    async fn interrupt(&self, _tenant_id: &str, _session_id: &Uuid) -> Result<(), TenantError> {
        Ok(())
    }

    async fn subscribe_events(
        &self,
        _tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<mpsc::Receiver<agent_core::AgentEvent>, TenantError> {
        let (tx, rx) = mpsc::channel(32);
        self.event_senders
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_default()
            .push(tx);
        Ok(rx)
    }

    async fn delete_session(&self, _tenant_id: &str, session_id: &Uuid) -> Result<(), TenantError> {
        self.sessions
            .lock()
            .unwrap()
            .remove(&session_id.to_string());
        Ok(())
    }

    async fn update_session(
        &self,
        _tenant_id: &str,
        session_id: &Uuid,
        updates: SessionUpdates,
    ) -> Result<SessionInfo, TenantError> {
        let mut sessions = self.sessions.lock().unwrap();
        let info = sessions
            .get_mut(&session_id.to_string())
            .ok_or_else(|| TenantError::SessionNotFound(session_id.to_string()))?;
        if let Some(title) = updates.title {
            info.title = title;
        }
        if let Some(model) = updates.model {
            info.model = model;
        }
        if let Some(system_prompt) = updates.system_prompt {
            info.system_prompt = Some(system_prompt);
        }
        Ok(info.clone())
    }

    async fn compact_session(
        &self,
        _tenant_id: &str,
        _session_id: &Uuid,
    ) -> Result<(), TenantError> {
        Ok(())
    }

    async fn get_session_messages(
        &self,
        _tenant_id: &str,
        _session_id: &Uuid,
    ) -> Result<Vec<agent_core::AgentMessage>, TenantError> {
        Ok(vec![])
    }

    async fn get_session_state(
        &self,
        _tenant_id: &str,
        _session_id: &Uuid,
    ) -> Result<(agent_core::SessionState, Option<String>), TenantError> {
        Ok((agent_core::SessionState::Idle, None))
    }

    async fn get_quota(&self, tenant_id: &str) -> Result<tenant::manager::QuotaInfo, TenantError> {
        Ok(tenant::manager::QuotaInfo {
            tenant_id: tenant_id.into(),
            max_concurrent_sessions: 10,
            active_sessions: 0,
            max_tokens_per_day: 1_000_000,
            tokens_used_today: 0,
            max_tool_calls_per_minute: 60,
            tool_calls_in_last_minute: 0,
            default_model: "claude-sonnet-4".into(),
            available_models: vec!["claude-sonnet-4".into()],
        })
    }

    async fn batch_create_sessions(
        &self,
        tenant_id: &str,
        count: usize,
        template: CreateSessionParams,
    ) -> Result<tenant::manager::BatchCreateResult, TenantError> {
        let mut created = vec![];
        for _ in 0..count {
            created.push(self.create_session(tenant_id, template.clone()).await?);
        }
        Ok(tenant::manager::BatchCreateResult {
            created,
            failed: vec![],
        })
    }

    async fn clone_session(
        &self,
        tenant_id: &str,
        _session_id: &Uuid,
        title: Option<String>,
    ) -> Result<SessionInfo, TenantError> {
        self.create_session(
            tenant_id,
            CreateSessionParams {
                title,
                ..Default::default()
            },
        )
        .await
    }

    async fn reset_session(
        &self,
        _tenant_id: &str,
        _session_id: &Uuid,
    ) -> Result<agent_core::SessionState, TenantError> {
        Ok(agent_core::SessionState::Idle)
    }

    async fn send_message_and_wait(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        content: Vec<ai_provider::Content>,
        _timeout_ms: u64,
    ) -> Result<tenant::manager::WaitResult, TenantError> {
        let turn_index = self.send_message(tenant_id, session_id, content).await?;
        Ok(tenant::manager::WaitResult::Timeout { turn_index })
    }

    async fn shutdown(&self) {}

    fn active_session_count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }
}

/// Build a test router with the mock tenant manager.
/// NOTE: The router still uses Aspectus auth middleware (default feature).
/// For tests that call the router directly, use `test_token` to pass a valid-looking token.
pub fn test_router() -> axum::Router {
    use api_gateway::config::AspectusConfig;
    let manager = Arc::new(MockTenantManager::new()) as Arc<dyn TenantManager>;
    let registry = Arc::new(tenant::TenantRegistry::new());
    let config = ServerConfig::default();
    let aspectus_config = AspectusConfig {
        base_url: "http://localhost:1".into(),
        service_token: "test".into(),
        timeout_ms: 100,
    };
    let state = Arc::new(AppState::new(manager, config, registry, &aspectus_config).unwrap());
    api_gateway::build_router(state)
}

/// Simple test token string for use in Authorization headers.
pub fn test_token(_tenant_id: &str) -> String {
    "pk_live_test".to_string()
}
