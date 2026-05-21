use std::collections::HashMap;
use std::sync::Arc;

use crate::space::AgentSpace;

/// Configurable policy fields for `DefaultHookDispatcher`.
#[derive(Clone)]
pub struct DefaultHookConfig {
    pub denied_tools: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub path_guard_fields: HashMap<String, Vec<String>>,
    pub path_guard_scan_unknown: bool,
    pub max_turns_per_session: usize,
    /// Optional media-cost callback.  The first argument is `tenant_id`, the
    /// second is the cost in dollars.
    pub cost_callback: Option<Arc<dyn Fn(&str, f64) + Send + Sync>>,
}

impl std::fmt::Debug for DefaultHookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultHookConfig")
            .field("denied_tools", &self.denied_tools)
            .field("allowed_tools", &self.allowed_tools)
            .field("path_guard_fields", &self.path_guard_fields)
            .field("path_guard_scan_unknown", &self.path_guard_scan_unknown)
            .field("max_turns_per_session", &self.max_turns_per_session)
            .field("cost_callback", &self.cost_callback.is_some())
            .finish()
    }
}

impl Default for DefaultHookConfig {
    fn default() -> Self {
        Self {
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            path_guard_fields: HashMap::new(),
            path_guard_scan_unknown: false,
            max_turns_per_session: 0,
            cost_callback: None,
        }
    }
}

/// Global runtime configuration that aggregates all infrastructure
/// dependencies needed to build a `SessionActor`.
///
/// Constructed once at server startup (e.g. in `api-gateway`) and passed
/// to `TenantManagerImpl::new()`.
#[derive(Clone)]
pub struct RuntimeConfig {
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

    // Runtime defaults
    pub compaction_config: crate::harness::compaction::CompactionConfig,
    pub agent_space: AgentSpace,
    pub hook_config: DefaultHookConfig,
    pub memory_store: Option<Arc<dyn crate::memory::MemoryStore>>,
}

impl std::fmt::Debug for RuntimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeConfig")
            .field("default_model", &self.default_model)
            .field("default_system_prompt", &self.default_system_prompt)
            .field("default_context_window", &self.default_context_window)
            .field("store", &self.store.is_some())
            .field("media_provider", &self.media_provider.is_some())
            .field("media_registry", &self.media_registry.is_some())
            .field("http_client", &self.http_client)
            .field("compaction_config", &self.compaction_config)
            .field("agent_space", &self.agent_space)
            .field("hook_config", &self.hook_config)
            .finish()
    }
}

impl Default for RuntimeConfig {
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
            compaction_config: crate::harness::compaction::CompactionConfig::default(),
            agent_space: AgentSpace::default(),
            hook_config: DefaultHookConfig::default(),
            memory_store: None,
        }
    }
}
