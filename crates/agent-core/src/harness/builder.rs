use std::collections::HashSet;
use std::sync::Arc;

use crate::error::AgentError;
use crate::file_ops::DefaultFileOperationExtractor;
use crate::harness::compaction::Compactor;
use crate::harness::session::{SessionActor, SessionConfig};
use crate::hook::combined::CombinedDispatcher;
use crate::hook::default_dispatcher::DefaultHookDispatcher;
use crate::hook::dispatcher::HookDispatcher;
use crate::memory::hook::MemoryHookDispatcher;
use crate::skills::FileSystemSkillLoader;
use crate::skills::loader::SkillLoader;
use crate::space::AgentSpace;
use crate::tools::{HttpProxyTool, MediaGenerationTool, ToolConfig};
use crate::types::{AgentTool, AgentToolRef};

use super::config::HarnessConfig;

/// Result of a successful `SessionBuilder::build()` call.
pub struct BuiltSession {
    pub actor: SessionActor,
    pub tools: Vec<AgentToolRef>,
}

/// Builder that assembles a `SessionActor` and its tools from a `HarnessConfig`.
///
/// Usage:
/// ```rust,ignore
/// let built = SessionBuilder::new(&harness_config)
///     .tenant_id("acme")
///     .session_id("uuid")
///     .system_prompt("You are a helpful assistant.")
///     .model("gpt-4")
///     .with_builtin_tools(vec![bash, read, write])
///     .with_external_tools(params.tools)
///     .build()
///     .await?;
/// ```
pub struct SessionBuilder {
    config: HarnessConfig,
    tenant_id: String,
    session_id: String,
    system_prompt: String,
    model: String,
    builtin_tools: Vec<AgentToolRef>,
    external_tools: Vec<ToolConfig>,
}

impl SessionBuilder {
    /// Start a new builder from a `HarnessConfig`.
    pub fn new(config: &HarnessConfig) -> Self {
        Self {
            config: config.clone(),
            tenant_id: String::new(),
            session_id: String::new(),
            system_prompt: config.default_system_prompt.clone(),
            model: config.default_model.clone(),
            builtin_tools: Vec::new(),
            external_tools: Vec::new(),
        }
    }

    /// Set the tenant identifier.
    pub fn tenant_id(mut self, id: impl Into<String>) -> Self {
        self.tenant_id = id.into();
        self
    }

    /// Set the session identifier.
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = id.into();
        self
    }

    /// Set the system prompt for this session.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Set the LLM model for this session.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Register built-in tools implemented in-process.
    ///
    /// Builtins have the lowest priority: external (Tavern) tools and media
    /// generation are placed before them in the tool list. When the agent loop
    /// looks up a tool by name, the first match wins — so externals effectively
    /// shadow builtins with the same name.
    pub fn with_builtin_tools(mut self, tools: Vec<AgentToolRef>) -> Self {
        self.builtin_tools = tools;
        self
    }

    /// Append external HTTP proxy tools to the session.
    pub fn with_external_tools(mut self, tools: Vec<ToolConfig>) -> Self {
        self.external_tools = tools;
        self
    }

    /// Assemble the session.
    ///
    /// This method:
    /// 1. Creates a `DefaultHookDispatcher` from `HarnessConfig.hook_config`.
    /// 2. Builds the tool list (media generation + HTTP proxies).
    /// 3. Creates a `Compactor`.
    /// 4. Loads skills for the tenant.
    /// 5. Instantiates `SessionActor`.
    pub async fn build(self) -> Result<BuiltSession, AgentError> {
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();

        // 1. Hook dispatcher
        let dispatcher = DefaultHookDispatcher::from_config(
            self.config.agent_space.clone(),
            &self.config.hook_config,
        );

        let hook_dispatcher: Arc<dyn HookDispatcher> =
            if let Some(ref mem) = self.config.memory_store {
                Arc::new(CombinedDispatcher::new(vec![
                    Arc::new(dispatcher),
                    Arc::new(MemoryHookDispatcher::new(
                        mem.clone(),
                        self.model.clone(),
                        std::time::SystemTime::now(),
                    )),
                ]))
            } else {
                Arc::new(dispatcher)
            };

        // 2. Tool assembly: external → media → builtin
        //    Earlier entries win on `iter().find()` name lookup, so highest
        //    priority tools go first. External (Tavern) tools have highest
        //    priority, then media generation, then builtins.
        let mut tools: Vec<AgentToolRef> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        // 2a. External HTTP proxy tools (first — highest priority)
        for tool_config in &self.external_tools {
            let proxy = Arc::new(HttpProxyTool::new(
                tool_config.clone(),
                tenant_id.clone(),
                session_id.clone(),
                self.config.http_client.clone(),
            ));
            let name = proxy.name().to_string();
            if seen.contains(&name) {
                tracing::info!(%name, "external tool name collision, keeping first");
                continue;
            }
            seen.insert(name);
            tools.push(proxy);
        }

        // 2b. Media generation tool (if configured)
        if let (Some(media_provider), Some(media_registry)) =
            (&self.config.media_provider, &self.config.media_registry)
        {
            let media_tool = Arc::new(MediaGenerationTool::new(
                media_provider.clone(),
                media_registry.clone(),
                self.model.clone(),
                &tenant_id,
            ));
            let name = media_tool.name().to_string();
            if seen.contains(&name) {
                tracing::warn!(%name, "media tool shadowed, skipping");
            } else {
                seen.insert(name);
                tools.push(media_tool);
            }
        }

        // 2c. Built-in tools (last — lowest priority)
        for tool in &self.builtin_tools {
            let name = tool.name().to_string();
            if seen.contains(&name) {
                tracing::warn!(%name, "builtin tool shadowed, skipping");
                continue;
            }
            seen.insert(name);
            tools.push(tool.clone());
        }

        // 3. Compaction actor
        let compaction_actor = Arc::new(Compactor::new(
            self.config.compaction_config.clone(),
            self.config.provider.clone(),
            self.model.clone(),
            Arc::new(DefaultFileOperationExtractor::default()),
        ));

        // 4. Skills
        let skills = Self::load_skills(&self.config.agent_space, &tenant_id).await?;

        // 5. Session actor
        let actor = SessionActor::new(SessionConfig {
            tenant_id,
            session_id: session_id.clone(),
            system_prompt: self.system_prompt,
            model: self.model,
            provider: self.config.provider.clone(),
            hook_dispatcher,
            compaction_actor,
            tools: tools.clone(),
            store: self.config.store.clone(),
            skills,
        });

        Ok(BuiltSession { actor, tools })
    }

    async fn load_skills(
        agent_space: &AgentSpace,
        tenant_id: &str,
    ) -> Result<Vec<crate::skills::Skill>, AgentError> {
        let user_skills_dir = agent_space.skills_dir().display().to_string();
        let project_skills_dir = agent_space.workspace_for(tenant_id).join("skills");
        let _ = tokio::fs::create_dir_all(&project_skills_dir).await;

        let loader = FileSystemSkillLoader {
            user_skills_dir,
            project_skills_dir: project_skills_dir.display().to_string(),
            explicit_paths: Vec::new(),
        };
        let result = loader.load_skills().await;
        if !result.diagnostics.is_empty() {
            for diag in &result.diagnostics {
                tracing::warn!(
                    path = %diag.path,
                    kind = ?diag.kind,
                    "skill diagnostic: {}",
                    diag.message
                );
            }
        }
        Ok(result.skills)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::compaction::CompactionConfig;
    use crate::space::AgentSpace;

    fn dummy_runtime_config() -> HarnessConfig {
        HarnessConfig {
            provider: Arc::new(ai_provider::RouterProvider::new()),
            default_model: "gpt-4".to_string(),
            default_system_prompt: "You are a helper.".to_string(),
            default_context_window: 128_000,
            store: None,
            media_provider: None,
            media_registry: None,
            http_client: reqwest::Client::new(),
            available_models: vec!["gpt-4".to_string()],
            compaction_config: CompactionConfig::default(),
            agent_space: AgentSpace::default(),
            hook_config: crate::harness::config::HookConfig::default(),
            memory_store: None,
        }
    }

    #[tokio::test]
    async fn test_session_builder_basic() {
        let config = dummy_runtime_config();
        let built = SessionBuilder::new(&config)
            .tenant_id("test-tenant")
            .session_id("sess-1")
            .system_prompt("Be helpful.")
            .model("gpt-4")
            .build()
            .await
            .expect("build should succeed");

        assert_eq!(built.actor.tenant_id(), "test-tenant");
        assert_eq!(built.actor.session_id(), "sess-1");
        assert_eq!(built.actor.system_prompt(), "Be helpful.");
        assert!(built.tools.is_empty());
    }

    #[tokio::test]
    async fn test_session_builder_with_external_tools() {
        let config = dummy_runtime_config();
        let tool = ToolConfig {
            name: "echo".to_string(),
            description: "echo".to_string(),
            parameters: serde_json::json!({}),
            endpoint: "https://example.com/echo".to_string(),
            timeout_ms: None,
            headers: None,
        };
        let built = SessionBuilder::new(&config)
            .tenant_id("test-tenant")
            .session_id("sess-1")
            .with_external_tools(vec![tool])
            .build()
            .await
            .expect("build should succeed");

        assert_eq!(built.tools.len(), 1);
        assert_eq!(built.tools[0].name(), "echo");
    }

    #[tokio::test]
    async fn test_session_builder_with_builtin_tools() {
        use crate::test_utils::TestTool;

        let config = dummy_runtime_config();
        let built = SessionBuilder::new(&config)
            .tenant_id("test-tenant")
            .session_id("sess-1")
            .with_builtin_tools(vec![Arc::new(TestTool::new(
                "echo",
                "echoes",
                serde_json::json!({}),
            ))])
            .build()
            .await
            .expect("build should succeed");

        assert_eq!(built.tools.len(), 1);
        assert_eq!(built.tools[0].name(), "echo");
    }

    #[tokio::test]
    async fn test_session_builder_builtin_shadowed_by_external() {
        use crate::test_utils::TestTool;

        let config = dummy_runtime_config();
        let external = ToolConfig {
            name: "echo".to_string(),
            description: "external echo".to_string(),
            parameters: serde_json::json!({}),
            endpoint: "https://example.com/echo".to_string(),
            timeout_ms: None,
            headers: None,
        };
        let built = SessionBuilder::new(&config)
            .tenant_id("test-tenant")
            .session_id("sess-1")
            .with_builtin_tools(vec![Arc::new(TestTool::new(
                "echo",
                "builtin echo",
                serde_json::json!({}),
            ))])
            .with_external_tools(vec![external])
            .build()
            .await
            .expect("build should succeed");

        // External registered first, shadows builtin by name collision.
        // Only the external tool appears in the list.
        assert_eq!(built.tools.len(), 1, "external should shadow builtin");
        assert_eq!(built.tools[0].name(), "echo");
    }
}
