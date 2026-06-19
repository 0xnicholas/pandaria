use std::collections::HashMap;
use std::sync::Arc;

use crate::space::AgentSpace;

/// Configurable policy fields for `DefaultHookDispatcher`.
#[derive(Clone, Default)]
pub struct HookConfig {
    pub denied_tools: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub path_guard_fields: HashMap<String, Vec<String>>,
    pub path_guard_scan_unknown: bool,
    pub max_turns_per_session: usize,
    /// Optional media-cost callback.  The first argument is `tenant_id`, the
    /// second is the cost in dollars.
    pub cost_callback: Option<Arc<dyn Fn(&str, f64) + Send + Sync>>,
}

impl HookConfig {
    /// Populate `path_guard_fields` with Pawbun tool field mappings
    /// for the dual-layer sandbox defense.
    pub fn with_pawbun_defaults(mut self) -> Self {
        self.path_guard_fields
            .insert("file_read".into(), vec!["path".into()]);
        self.path_guard_fields
            .insert("file_write".into(), vec!["path".into()]);
        self.path_guard_fields
            .insert("directory_list".into(), vec!["path".into()]);
        self.path_guard_fields
            .insert("code_execute".into(), vec!["work_dir".into()]);
        self
    }
}

impl std::fmt::Debug for HookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookConfig")
            .field("denied_tools", &self.denied_tools)
            .field("allowed_tools", &self.allowed_tools)
            .field("path_guard_fields", &self.path_guard_fields)
            .field("path_guard_scan_unknown", &self.path_guard_scan_unknown)
            .field("max_turns_per_session", &self.max_turns_per_session)
            .field("cost_callback", &self.cost_callback.is_some())
            .finish()
    }
}

/// Global harness configuration that aggregates all infrastructure
/// dependencies needed to build a `SessionActor`.
///
/// Constructed once at server startup (e.g. in `api-gateway`) and passed
/// to `TenantManagerImpl::new()`.
#[derive(Clone)]
pub struct HarnessConfig {
    pub provider: Arc<dyn ai_provider::LlmProvider>,
    pub default_model: String,
    pub default_system_prompt: String,
    /// Reserved for future LLM-selection logic.
    pub default_context_window: usize,

    // Optional infrastructure
    pub store: Option<Arc<dyn crate::persistence::SessionStore>>,
    pub media_provider: Option<Arc<dyn ai_provider::MediaProvider>>,
    pub media_registry: Option<Arc<ai_provider::MediaModelRegistry>>,

    // Shared HTTP client for external tool proxies and webhooks.
    pub http_client: reqwest::Client,

    /// Models available for this runtime (returned in quota queries).
    pub available_models: Vec<String>,

    // Runtime defaults
    pub compaction_config: crate::harness::compaction::CompactionConfig,
    pub agent_space: AgentSpace,
    pub hook_config: HookConfig,
    pub memory_store: Option<Arc<dyn crate::memory::MemoryStore>>,

    /// Days to retain completed/failed sessions before cleanup (default: 7).
    pub session_retention_days: u32,
    /// Hours between cleanup task executions (default: 24).
    pub session_cleanup_interval_hours: u32,
}

impl std::fmt::Debug for HarnessConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HarnessConfig")
            .field("default_model", &self.default_model)
            .field("default_system_prompt", &self.default_system_prompt)
            .field("default_context_window", &self.default_context_window)
            .field("store", &self.store.is_some())
            .field("media_provider", &self.media_provider.is_some())
            .field("media_registry", &self.media_registry.is_some())
            .field("http_client", &self.http_client)
            .field("available_models", &self.available_models)
            .field("compaction_config", &self.compaction_config)
            .field("agent_space", &self.agent_space)
            .field("hook_config", &self.hook_config)
            .field("session_retention_days", &self.session_retention_days)
            .field(
                "session_cleanup_interval_hours",
                &self.session_cleanup_interval_hours,
            )
            .finish()
    }
}

impl HarnessConfig {
    /// Build a `HarnessConfig` from environment variables.
    ///
    /// `provider` must be supplied by the caller (it cannot be constructed
    /// from env vars alone). All other fields read from environment variables
    /// with sensible defaults.
    ///
    /// # Environment Variables
    ///
    /// | Variable | Default | Description |
    /// |---|---|---|
    /// | `PANDARIA_DEFAULT_MODEL` | `deepseek/deepseek-v4-pro` | Default model for new sessions |
    /// | `PANDARIA_DEFAULT_SYSTEM_PROMPT` | `You are a helpful assistant.` | Default system prompt |
    /// | `PANDARIA_DEFAULT_CONTEXT_WINDOW` | `128000` | Default context window size |
    /// | `PANDARIA_AVAILABLE_MODELS` | (derived from `default_model`) | Comma-separated model list |
    /// | `PANDARIA_COMPACTION_ENABLED` | `true` | Enable automatic compaction |
    /// | `PANDARIA_COMPACTION_RESERVE_TOKENS` | `4096` | Tokens reserved for response |
    /// | `PANDARIA_COMPACTION_KEEP_RECENT_TOKENS` | `8192` | Recent context tokens to keep |
    pub fn from_env(provider: Arc<dyn ai_provider::LlmProvider>) -> Self {
        let default_model = std::env::var("PANDARIA_DEFAULT_MODEL")
            .unwrap_or_else(|_| "deepseek/deepseek-v4-pro".to_string());

        let default_system_prompt = std::env::var("PANDARIA_DEFAULT_SYSTEM_PROMPT")
            .unwrap_or_else(|_| "You are a helpful assistant.".to_string());

        let default_context_window = std::env::var("PANDARIA_DEFAULT_CONTEXT_WINDOW")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(128_000);

        let available_models: Vec<String> = std::env::var("PANDARIA_AVAILABLE_MODELS")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| s.split(',').map(|m| m.trim().to_string()).collect())
            .unwrap_or_else(|| vec![default_model.clone()]);

        let compaction_enabled = std::env::var("PANDARIA_COMPACTION_ENABLED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(true);

        let compaction_reserve = std::env::var("PANDARIA_COMPACTION_RESERVE_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4096);

        let compaction_keep = std::env::var("PANDARIA_COMPACTION_KEEP_RECENT_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8192);

        let session_retention_days = std::env::var("PANDARIA_SESSION_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(7);

        let session_cleanup_interval_hours =
            std::env::var("PANDARIA_SESSION_CLEANUP_INTERVAL_HOURS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(24);

        Self {
            provider,
            default_model,
            default_system_prompt,
            default_context_window,
            store: None,
            media_provider: None,
            media_registry: None,
            http_client: reqwest::Client::new(),
            available_models,
            compaction_config: crate::harness::compaction::CompactionConfig::new(
                compaction_enabled,
                compaction_reserve,
                compaction_keep,
            ),
            agent_space: AgentSpace::from_env_or_default(),
            hook_config: HookConfig::default(),
            memory_store: None,
            session_retention_days,
            session_cleanup_interval_hours,
        }
    }
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            provider: Arc::new(ai_provider::RouterProvider::new()),
            default_model: String::new(),
            default_system_prompt: String::new(),
            default_context_window: 128_000,
            store: None,
            media_provider: None,
            media_registry: None,
            http_client: reqwest::Client::new(),
            available_models: Vec::new(),
            compaction_config: crate::harness::compaction::CompactionConfig::default(),
            agent_space: AgentSpace::default(),
            hook_config: HookConfig::default(),
            memory_store: None,
            session_retention_days: 7,
            session_cleanup_interval_hours: 24,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_hook_config_with_pawbun_defaults() {
        let config = HookConfig::default().with_pawbun_defaults();
        assert_eq!(
            config.path_guard_fields.get("file_read").unwrap(),
            &vec!["path".to_string()]
        );
        assert_eq!(
            config.path_guard_fields.get("file_write").unwrap(),
            &vec!["path".to_string()]
        );
        assert_eq!(
            config.path_guard_fields.get("directory_list").unwrap(),
            &vec!["path".to_string()]
        );
        assert_eq!(
            config.path_guard_fields.get("code_execute").unwrap(),
            &vec!["work_dir".to_string()]
        );
    }

    #[test]
    fn test_hook_config_with_pawbun_preserves_existing() {
        let config = HookConfig {
            path_guard_fields: {
                let mut m = HashMap::new();
                m.insert("custom".into(), vec!["file".into()]);
                m
            },
            ..Default::default()
        }
        .with_pawbun_defaults();

        assert!(config.path_guard_fields.contains_key("custom"));
        assert!(config.path_guard_fields.contains_key("file_read"));
    }
}
