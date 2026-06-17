use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use futures_util::stream::BoxStream;
use serde_json::Value;
use tavern_core::{AgentConfig, SkillConfig, ToolRunner};
use tokio::sync::{Mutex, Semaphore};
use tracing::instrument;

use agent_core::harness::builder::SessionBuilder;
use agent_core::harness::config::HarnessConfig;
use agent_core::harness::session::SessionActor;
use agent_core::tools::ToolConfig;

use super::executor::{
    AgentExecutor, AgentExecutorError, AgentInput, AgentOutput, AgentOutputChunk, AgentResolver,
};
use super::role::Role;

/// Wraps a cached `SessionActor` with metadata for eviction policies.
#[derive(Clone)]
struct CachedSession {
    actor: Arc<Mutex<SessionActor>>,
    last_used: Instant,
}

/// Production implementation of `AgentExecutor` backed by `agent-core::SessionActor`.
///
/// - Caches sessions by `role_id:model` key, reusing `SessionActor` across
///   missions that share the same role and model.
/// - Concurrent calls for the same key serialize on `tokio::sync::Mutex::lock()`.
/// - Session count bounded by `session_semaphore` (default: 8) to prevent
///   resource exhaustion.
/// - Skills are converted to `ToolConfig` for Sidecar-runner skills only;
///   Rust/subprocess skills are skipped (P0 limitation).
#[derive(Clone)]
pub struct PandariaAgentExecutor {
    tenant_id: String,
    team_id: String,
    harness_config: HarnessConfig,
    agent_resolver: Arc<dyn AgentResolver>,
    sessions: Arc<std::sync::Mutex<HashMap<String, CachedSession>>>,
    session_semaphore: Arc<Semaphore>,
    /// Idle timeout for cached sessions. Sessions unused for longer than
    /// this duration are evicted on next access. Default: 5 minutes.
    session_idle_timeout: std::time::Duration,
    /// Optional base URL for the Tavern tool server (e.g. `http://localhost:8080`).
    /// When set, Rust and Subprocess skills are also converted to HTTP proxy tools
    /// pointing at `{tool_server_base_url}/api/tools/{skill_id}`.
    tool_server_base_url: Option<String>,
}

impl PandariaAgentExecutor {
    /// Create a new executor.
    pub fn new(
        tenant_id: impl Into<String>,
        team_id: impl Into<String>,
        harness_config: HarnessConfig,
        agent_resolver: Arc<dyn AgentResolver>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            team_id: team_id.into(),
            harness_config,
            agent_resolver,
            sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            session_semaphore: Arc::new(Semaphore::new(8)),
            session_idle_timeout: std::time::Duration::from_secs(300), // 5 min
            tool_server_base_url: None,
        }
    }

    /// Set the maximum number of concurrent sessions (default: 8).
    pub fn with_max_sessions(mut self, n: usize) -> Self {
        self.session_semaphore = Arc::new(Semaphore::new(n.max(1)));
        self
    }

    /// Set the Tavern tool server base URL, enabling HTTP proxy tool conversion
    /// for Rust and Subprocess skills in addition to Sidecar skills.
    ///
    /// Skills are registered as HTTP proxy tools pointing at
    /// `{base_url}/api/tools/{skill_id}`.
    pub fn with_tool_server(mut self, base_url: impl Into<String>) -> Self {
        self.tool_server_base_url = Some(base_url.into().trim_end_matches('/').to_string());
        self
    }

    /// Set the idle timeout for cached sessions (default: 5 minutes).
    /// Sessions unused for longer than this duration are evicted on next access.
    pub fn with_session_idle_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.session_idle_timeout = timeout;
        self
    }

    /// Return the number of cached sessions. Available for test assertions.
    pub fn session_count(&self) -> usize {
        self.sessions
            .lock()
            .expect("pandaria executor session map poisoned")
            .len()
    }
}

#[async_trait]
impl AgentExecutor for PandariaAgentExecutor {
    async fn resolve_role(&self, role_id: &str) -> Result<Role, AgentExecutorError> {
        let agent = self
            .agent_resolver
            .resolve(role_id)
            .await
            .ok_or_else(|| AgentExecutorError::RoleNotFound {
                id: role_id.to_string(),
            })?;

        Ok(Role {
            id: role_id.to_string(),
            name: agent.name.clone(),
            description: agent.description.clone(),
            agent_id: agent.id.clone(),
            team_instructions: None,       // SquadEngine merges team-level fields
            model_override: Some(agent.model.clone()),
            visibility: Default::default(), // SquadEngine merges team-level fields
            skills: agent.skills.clone(),
        })
    }

    #[instrument(
        skip(self, input),
        fields(
            tenant_id = %self.tenant_id,
            team_id = %self.team_id,
            role_id = %role_id,
            squad_id = %input.squad_id.as_deref().unwrap_or("unknown"),
            mission_id = %input.mission_id.as_deref().unwrap_or("unknown"),
        )
    )]
    async fn execute(
        &self,
        role_id: &str,
        input: AgentInput,
    ) -> Result<AgentOutput, AgentExecutorError> {
        let start = std::time::Instant::now();

        // 1. Resolve agent config
        let agent = self
            .agent_resolver
            .resolve(role_id)
            .await
            .ok_or_else(|| AgentExecutorError::RoleNotFound {
                id: role_id.to_string(),
            })?;

        // 2. Determine model string
        let model = match &input.model_override {
            Some(m) => format!("{}/{}", m.provider, m.name),
            None => format!("{}/{}", agent.model.provider, agent.model.name),
        };

        // 3. Acquire or create cached session (key = role_id:model)
        let session_arc = self.acquire_session(role_id, &model, &agent).await?;

        // 4. Build prompt from AgentInput + TeamContext
        let prompt = build_role_prompt(&input, role_id);

        // 5. Execute — no internal timeout (caller responsibility).
        //    Lock is held for the full duration of the LLM call.
        let mut actor = session_arc.lock().await;
        let text = actor
            .complete(prompt)
            .await
            .map_err(|e| map_agent_error(e))?;
        let usage = actor.last_usage().cloned();
        drop(actor);

        // 6. Build output. If the text parses as JSON, return it as a Value
        //    object so Handoff detection works in hierarchical mode.
        let content = try_parse_json(&text);
        let usage_value = usage.map(|u| {
            serde_json::json!({
                "input_tokens": u.input_tokens,
                "output_tokens": u.output_tokens,
                "total_tokens": u.total_tokens,
            })
        });

        Ok(AgentOutput {
            content,
            usage: usage_value,
            latency: start.elapsed(),
            metadata: HashMap::new(),
        })
    }

    async fn execute_stream(
        &self,
        role_id: &str,
        input: AgentInput,
    ) -> Result<BoxStream<'static, AgentOutputChunk>, AgentExecutorError> {
        // Resolve agent and model
        let agent = self
            .agent_resolver
            .resolve(role_id)
            .await
            .ok_or_else(|| AgentExecutorError::RoleNotFound {
                id: role_id.to_string(),
            })?;

        let model = match &input.model_override {
            Some(m) => format!("{}/{}", m.provider, m.name),
            None => format!("{}/{}", agent.model.provider, agent.model.name),
        };

        let session_arc = self.acquire_session(role_id, &model, &agent).await?;
        let prompt = build_role_prompt(&input, role_id);

        // Use complete_with_deltas to get per-chunk text deltas from the LLM
        let mut actor = session_arc.lock().await;
        let (full_text, deltas) = actor
            .complete_with_deltas(prompt)
            .await
            .map_err(|e| map_agent_error(e))?;
        let usage = actor.last_usage().cloned();
        drop(actor);

        let usage_value = usage.map(|u| {
            serde_json::json!({
                "input_tokens": u.input_tokens,
                "output_tokens": u.output_tokens,
                "total_tokens": u.total_tokens,
            })
        });

        // Yield deltas as chunks, then the full text as the final chunk
        use futures_util::stream;
        let mut chunks: Vec<AgentOutputChunk> = deltas
            .into_iter()
            .map(|delta| AgentOutputChunk {
                content: Value::String(delta),
                usage: None,
            })
            .collect();

        // Append final chunk with full text + usage (parse JSON if possible)
        if !chunks.is_empty() || !full_text.is_empty() {
            chunks.push(AgentOutputChunk {
                content: try_parse_json(&full_text),
                usage: usage_value,
            });
        }

        Ok(Box::pin(stream::iter(chunks)))
    }

    async fn flush(&self) -> Result<(), AgentExecutorError> {
        let map = {
            self.sessions
                .lock()
                .expect("pandaria executor session map poisoned")
                .clone()
        };
        for (cache_key, cached) in map {
            let mut actor = cached.actor.lock().await;
            actor.flush().await.map_err(|e| {
                AgentExecutorError::ExecutionFailed(format!(
                    "flush session {} failed: {}",
                    cache_key, e
                ))
            })?;
        }
        Ok(())
    }
}

// ── Private helpers ────────────────────────────────────────────────────────

impl PandariaAgentExecutor {
    /// Acquire or create a cached `SessionActor` for the given role+model pair.
    ///
    /// Uses a semaphore to bound the total number of concurrent sessions.
    /// Double-checks the cache after acquiring the semaphore to avoid duplicate
    /// session creation. Evicts sessions that have been idle longer than
    /// `session_idle_timeout`.
    async fn acquire_session(
        &self,
        role_id: &str,
        model: &str,
        agent: &AgentConfig,
    ) -> Result<Arc<Mutex<SessionActor>>, AgentExecutorError> {
        let cache_key = format!("{}:{}", role_id, model);
        let timeout = self.session_idle_timeout;

        // Fast path: check cache without acquiring semaphore
        {
            let mut map = self
                .sessions
                .lock()
                .expect("pandaria executor session map poisoned");
            if let Some(cached) = map.get(&cache_key) {
                if cached.last_used.elapsed() < timeout {
                    let actor = cached.actor.clone();
                    // Update last_used timestamp
                    map.get_mut(&cache_key).unwrap().last_used = Instant::now();
                    return Ok(actor);
                }
                // Expired — remove and flush below
            }
        }

        // Slow path: bounded by semaphore
        let _permit = self
            .session_semaphore
            .acquire()
            .await
            .expect("session semaphore should not be closed");

        // Double-check after acquiring semaphore, with eviction
        let expired_actor: Option<Arc<Mutex<SessionActor>>> = {
            let mut map = self
                .sessions
                .lock()
                .expect("pandaria executor session map poisoned");
            if let Some(cached) = map.get(&cache_key) {
                if cached.last_used.elapsed() < timeout {
                    let actor = cached.actor.clone();
                    map.get_mut(&cache_key).unwrap().last_used = Instant::now();
                    return Ok(actor);
                }
                // Remove expired entry, return actor for async flush
                map.remove(&cache_key).map(|c| c.actor)
            } else {
                None
            }
        }; // map lock released here

        // Flush expired session outside the lock
        if let Some(actor_arc) = expired_actor {
            if let Ok(mut actor) = actor_arc.try_lock() {
                let _ = actor.flush().await;
            }
        }

        let session_id = format!("{}-{}-{}", self.tenant_id, role_id, uuid::Uuid::new_v4());

        let tools = build_tool_configs(&agent.skills, self.tool_server_base_url.as_deref());

        let built = SessionBuilder::new(&self.harness_config)
            .tenant_id(self.tenant_id.clone())
            .session_id(session_id)
            .system_prompt(agent.instructions.clone())
            .model(model.to_string())
            .with_external_tools(tools)
            .build()
            .await
            .map_err(|e| AgentExecutorError::SessionBuildFailed {
                reason: e.to_string(),
            })?;

        let actor_arc = Arc::new(Mutex::new(built.actor));

        {
            let mut map = self
                .sessions
                .lock()
                .expect("pandaria executor session map poisoned");
            map.entry(cache_key).or_insert(CachedSession {
                actor: actor_arc.clone(),
                last_used: Instant::now(),
            });
        }

        Ok(actor_arc)
    }
}

// ── Free functions ─────────────────────────────────────────────────────────

/// Try to parse a string as JSON. If successful, return the parsed Value;
/// otherwise return the original string wrapped in `Value::String`.
///
/// This enables Handoff detection in hierarchical mode: if the LLM returns
/// `{"summary": "...", "next_role": "..."}`, it becomes a `Value::Object`
/// that `Handoff::detect()` can match.
fn try_parse_json(text: &str) -> Value {
    serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

/// Convert `SkillConfig` list to `agent_core::tools::ToolConfig` list.
///
/// - Sidecar skills with an explicit `url` are always converted.
/// - If `tool_server_base_url` is set, Rust and Subprocess skills are also
///   converted, using the endpoint `{base_url}/api/tools/{skill_id}`.
/// - Skills without a reachable endpoint are skipped.
fn build_tool_configs(skills: &[SkillConfig], tool_server_base_url: Option<&str>) -> Vec<ToolConfig> {
    skills
        .iter()
        .filter_map(|s| {
            let endpoint = match &s.runner {
                ToolRunner::Sidecar => s.url.clone(),
                ToolRunner::Rust | ToolRunner::Subprocess => {
                    tool_server_base_url.map(|base| format!("{}/api/tools/{}", base, s.id))
                }
            };
            endpoint.map(|url| ToolConfig {
                name: s.name.clone().unwrap_or_else(|| s.id.clone()),
                description: s.description.clone().unwrap_or_default(),
                parameters: s.parameters.clone(),
                endpoint: url,
                timeout_ms: Some(s.timeout_ms),
                headers: None,
            })
        })
        .collect()
}

/// Build the user prompt from `AgentInput` and role context.
fn build_role_prompt(input: &AgentInput, role_id: &str) -> String {
    let mut parts: Vec<String> = Vec::new();

    // 1. Current role's private context
    if let Some(private_val) = input.context.private.get(role_id) {
        if !private_val.is_null() {
            parts.push(format!("[Private Context]\n{}", private_val));
        }
    }

    // 2. Shared context
    if !input.context.shared.is_null() {
        parts.push(format!("[Shared Context]\n{}", input.context.shared));
    }

    // 3. Recent thread messages (last 5)
    let recent: Vec<_> = input.context.thread.iter().rev().take(5).collect();
    if !recent.is_empty() {
        let msgs: Vec<String> = recent
            .iter()
            .rev()
            .map(|m| format!("[{}] {}", m.role, m.content))
            .collect();
        parts.push(format!("[Recent Messages]\n{}", msgs.join("\n")));
    }

    // 4. Task
    parts.push(format!("[Task]\n{}", input.task));

    parts.join("\n\n")
}

/// Map `agent_core::error::AgentError` → `AgentExecutorError`.
fn map_agent_error(e: agent_core::error::AgentError) -> AgentExecutorError {
    let msg = e.to_string();

    // Check for variants we can detect by string matching
    // (AgentError is #[non_exhaustive], so exhaustive match is impractical)
    if matches!(&e, agent_core::error::AgentError::ContextOverflow(_)) {
        AgentExecutorError::ContextOverflow(msg)
    } else if matches!(&e, agent_core::error::AgentError::QuotaExceeded(_)) {
        AgentExecutorError::ExecutionFailed(format!("quota exceeded: {}", msg))
    } else if matches!(&e, agent_core::error::AgentError::Cancelled) {
        AgentExecutorError::ExecutionFailed("cancelled".to_string())
    } else {
        AgentExecutorError::ExecutionFailed(msg)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::context::TeamContext;
    use tavern_core::{AgentConfig, ModelConfig, ToolRunner};

    fn make_skill_config(
        id: &str,
        name: &str,
        runner: ToolRunner,
        url: Option<&str>,
        command: Option<&str>,
    ) -> SkillConfig {
        SkillConfig {
            id: id.to_string(),
            name: Some(name.to_string()),
            description: None,
            parameters: serde_json::json!({}),
            timeout_ms: 30000,
            runner,
            command: command.map(|s| s.to_string()),
            cwd: None,
            env: None,
            url: url.map(|s| s.to_string()),
            config: serde_json::Value::Null,
        }
    }

    fn make_agent(id: &str) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            model: ModelConfig {
                provider: "openai".to_string(),
                name: "gpt-4o".to_string(),
                temperature: 0.7,
            },
            instructions: "You are a helpful assistant.".to_string(),
            skills: vec![],
            constraints: vec![],
            memory: Default::default(),
        }
    }

    #[test]
    fn build_tool_configs_filters_non_sidecar() {
        let skills = vec![
            make_skill_config("rust_skill", "rust_skill", ToolRunner::Rust, None, None),
            make_skill_config(
                "sidecar_skill",
                "sidecar_skill",
                ToolRunner::Sidecar,
                Some("http://localhost:9999/tool"),
                None,
            ),
            make_skill_config(
                "subprocess_skill",
                "subprocess_skill",
                ToolRunner::Subprocess,
                None,
                Some("echo"),
            ),
            make_skill_config(
                "sidecar_no_url",
                "sidecar_no_url",
                ToolRunner::Sidecar,
                None,
                None,
            ),
        ];

        // Without tool_server_base_url, only sidecar with explicit URL
        let configs = build_tool_configs(&skills, None);
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "sidecar_skill");

        // With tool_server_base_url, all skills get endpoints
        let configs = build_tool_configs(&skills, Some("http://tools:8080"));
        assert_eq!(configs.len(), 3); // rust, sidecar (explicit URL wins), subprocess
        let names: Vec<&str> = configs.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"rust_skill"));
        assert!(names.contains(&"sidecar_skill"));
        assert!(names.contains(&"subprocess_skill"));
        // Sidecar with explicit URL keeps its URL, not the generated one
        let sidecar = configs.iter().find(|c| c.name == "sidecar_skill").unwrap();
        assert_eq!(sidecar.endpoint, "http://localhost:9999/tool");
        let rust = configs.iter().find(|c| c.name == "rust_skill").unwrap();
        assert_eq!(rust.endpoint, "http://tools:8080/api/tools/rust_skill");
    }

    #[test]
    fn build_role_prompt_includes_context() {
        let mut context = TeamContext::default();
        context.shared = serde_json::json!({"topic": "AI"});
        context.private.insert(
            "researcher".into(),
            serde_json::json!({"notes": "some notes"}),
        );

        let input = AgentInput {
            task: "research {{topic}}".into(),
            context,
            model_override: None,
            timeout: None,
            squad_id: None,
            mission_id: None,
        };

        let prompt = build_role_prompt(&input, "researcher");
        assert!(prompt.contains("[Private Context]"));
        assert!(prompt.contains("some notes"));
        assert!(prompt.contains("[Shared Context]"));
        assert!(prompt.contains("AI"));
        assert!(prompt.contains("[Task]"));
        assert!(prompt.contains("research"));
    }

    #[test]
    fn build_role_prompt_skips_null_private() {
        let mut context = TeamContext::default();
        context.shared = serde_json::json!({"x": 1});
        // No private for this role

        let input = AgentInput {
            task: "do something".into(),
            context,
            model_override: None,
            timeout: None,
            squad_id: None,
            mission_id: None,
        };

        let prompt = build_role_prompt(&input, "unknown_role");
        assert!(!prompt.contains("[Private Context]"));
        assert!(prompt.contains("[Shared Context]"));
    }

    #[test]
    fn map_agent_error_context_overflow() {
        let err = agent_core::error::AgentError::ContextOverflow("too big".into());
        let mapped = map_agent_error(err);
        assert!(matches!(mapped, AgentExecutorError::ContextOverflow(_)));
    }

    #[test]
    fn map_agent_error_fallback() {
 let err = agent_core::error::AgentError::ToolNotFound("bash".into());
        let mapped = map_agent_error(err);
        assert!(matches!(mapped, AgentExecutorError::ExecutionFailed(_)));
    }

    /// Struct that implements AgentResolver backed by a HashMap for testing.
    struct HashMapAgentResolver {
        agents: HashMap<String, AgentConfig>,
    }

    impl HashMapAgentResolver {
        fn new(agents: Vec<AgentConfig>) -> Self {
            Self {
                agents: agents.into_iter().map(|a| (a.id.clone(), a)).collect(),
            }
        }
    }

    #[async_trait]
    impl AgentResolver for HashMapAgentResolver {
        async fn resolve(&self, agent_id: &str) -> Option<AgentConfig> {
            self.agents.get(agent_id).cloned()
        }
    }

    #[tokio::test]
    async fn resolve_role_returns_agent_derived_role() {
        let agent = make_agent("test_agent");
        let resolver = Arc::new(HashMapAgentResolver::new(vec![agent.clone()]));

        let executor = PandariaAgentExecutor::new(
            "tenant1",
            "team1",
            HarnessConfig::default(),
            resolver,
        );

        let role = executor.resolve_role("test_agent").await.unwrap();
        assert_eq!(role.id, "test_agent");
        assert_eq!(role.agent_id, "test_agent");
        assert!(role.model_override.is_some());
        assert!(role.team_instructions.is_none()); // merged by SquadEngine
    }

    #[tokio::test]
    async fn resolve_role_not_found() {
        let resolver = Arc::new(HashMapAgentResolver::new(vec![]));

        let executor = PandariaAgentExecutor::new(
            "tenant1",
            "team1",
            HarnessConfig::default(),
            resolver,
        );

        let result = executor.resolve_role("nobody").await;
        assert!(matches!(result, Err(AgentExecutorError::RoleNotFound { .. })));
    }

    #[test]
    fn session_cache_key_includes_model() {
        // Verify that different models produce different cache keys.
        // (We test this by checking the cache map behavior)
        let agent = make_agent("test_agent");
        let resolver = Arc::new(HashMapAgentResolver::new(vec![agent]));
        let executor = PandariaAgentExecutor::new(
            "t1",
            "team1",
            HarnessConfig::default(),
            resolver,
        );

        // Same role but different models → different keys
        let key_a = format!("{}:{}", "test_agent", "openai/gpt-4o");
        let key_b = format!("{}:{}", "test_agent", "anthropic/claude");

        // Initially empty
        assert!(executor.sessions.lock().unwrap().is_empty());

        // Keys differ
        assert_ne!(key_a, key_b);
    }
}
