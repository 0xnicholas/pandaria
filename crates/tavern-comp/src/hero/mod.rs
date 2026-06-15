pub mod error;
pub mod hero;
pub mod loader;
pub mod registry;
pub mod validator;

pub use error::TavernError;
pub use hero::TavernHero;
pub use registry::AgentRegistry;

#[cfg(test)]
pub mod fixtures {
    use tavern_core::{AgentConfig, MemoryConfig, ModelConfig};

    pub fn default_agent() -> AgentConfig {
        AgentConfig {
            id: "test-agent".to_string(),
            name: "Test".to_string(),
            description: None,
            model: ModelConfig {
                provider: "openai".to_string(),
                name: "gpt-4o".to_string(),
                temperature: 0.7,
            },
            instructions: "test instructions".to_string(),
            skills: vec![],
            constraints: vec![],
            memory: MemoryConfig::default(),
        }
    }

    pub fn agent_with_id(id: &str) -> AgentConfig {
        let mut a = default_agent();
        a.id = id.to_string();
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hero::fixtures::agent_with_id;

    // ---------- Registry tests ----------

    #[test]
    fn test_registry_register_and_get() {
        let mut reg = AgentRegistry::new();
        let config = agent_with_id("a1");
        assert!(reg.register(config).is_ok());
        assert!(reg.get("a1").is_some());
        assert!(reg.get("a2").is_none());
    }

    #[test]
    fn test_registry_duplicate() {
        let mut reg = AgentRegistry::new();
        let config = agent_with_id("a1");
        reg.register(config.clone()).unwrap();
        let err = reg.register(config).unwrap_err();
        assert!(matches!(err, TavernError::DuplicateAgent { id } if id == "a1"));
    }

    #[test]
    fn test_registry_list_summary() {
        let mut reg = AgentRegistry::new();
        let mut config = agent_with_id("a1");
        config.description = Some("desc".to_string());
        reg.register(config).unwrap();
        let summaries = reg.list_summary();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "a1");
        assert_eq!(summaries[0].name, "Test");
        assert_eq!(summaries[0].description, Some("desc".to_string()));
    }

    // ---------- Loader tests ----------

    #[test]
    fn test_loader_valid_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(
            &path,
            r#"
id: writer
name: 写作助手
model:
  provider: openai
  name: gpt-4o
instructions: 写作
"#,
        )
        .unwrap();

        let config = loader::load_agent(&path).unwrap();
        assert_eq!(config.id, "writer");
        assert_eq!(config.name, "写作助手");
    }

    #[test]
    fn test_loader_invalid_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.yaml");
        std::fs::write(&path, "not: yaml: [").unwrap();

        let err = loader::load_agent(&path).unwrap_err();
        assert!(matches!(err, TavernError::ConfigParse { path, .. } if path.ends_with("bad.yaml")));
    }

    #[test]
    fn test_loader_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.yaml"),
            r#"
id: a
name: A
model:
  provider: openai
  name: gpt-4o
instructions: test
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("b.yml"),
            r#"
id: b
name: B
model:
  provider: openai
  name: gpt-4o
instructions: test
"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("ignore.txt"), "ignore").unwrap();

        let configs = loader::load_from_dir(dir.path()).unwrap();
        assert_eq!(configs.len(), 2);
    }

    // TODO: remove after verifying full test coverage

    #[tokio::test]
    async fn test_hero_execute_agent_not_found() {
        use std::sync::Arc;
        let runtime = Arc::new(crate::agent::AgentRuntime::new());
        let hero = TavernHero::new(runtime);

        let err = hero.execute("unknown", "task", None).await.unwrap_err();
        assert!(matches!(err, TavernError::AgentNotFound { id } if id == "unknown"));
    }

    #[tokio::test]
    async fn test_hero_execute_direct_with_mock_provider() {
        use std::sync::Arc;
        use ai_provider::test_utils::MockProvider;
        use crate::agent::AgentRuntime;
        use tavern_core::{AgentConfig, MemoryConfig, ModelConfig};

        // 1. Create a direct-mode runtime with a mock LLM
        let mock_provider = Arc::new(MockProvider::text("Hello from direct mode!"));
        let runtime = Arc::new(AgentRuntime::new_with_provider(mock_provider));
        let hero = TavernHero::new(runtime);

        // 2. Register a simple agent
        let config = AgentConfig {
            id: "test-agent".into(),
            name: "Test".into(),
            description: None,
            model: ModelConfig {
                provider: "mock".into(),
                name: "mock-model".into(),
                temperature: 0.7,
            },
            instructions: "You are a test agent.".into(),
            skills: vec![],
            constraints: vec![],
            memory: MemoryConfig::default(),
        };
        hero.register_agent(config).await.unwrap();

        // 3. Execute via execute() — auto-routes to direct path
        let result = hero.execute("test-agent", "Hello!", None).await;

        // 4. Verify response from mock LLM (no HTTP involved)
        assert!(result.is_ok(), "execute failed: {:?}", result.err());
        let value = result.unwrap();
        let text = value.as_str().unwrap();
        assert!(text.contains("Hello from direct mode!"), "Unexpected response: {}", text);
    }

    /// Real LLM test — requires DEEPSEEK_API_KEY in environment.
    /// Skip if key not set (returns Ok without running).
    #[tokio::test]
    async fn test_hero_execute_real_deepseek() {
        // Load .env from workspace root or pandaria dir
        let _ = dotenvy::from_filename("../../../pandaria/.env");
        let _ = dotenvy::dotenv();

        let api_key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
        if api_key.is_empty() {
            eprintln!("SKIP: DEEPSEEK_API_KEY not set");
            return;
        }

        use std::sync::Arc;
        use crate::agent::AgentRuntime;
        use tavern_core::{AgentConfig, MemoryConfig, ModelConfig};

        // Create direct-mode runtime (uses real DeepSeek API)
        let runtime = Arc::new(AgentRuntime::new());
        let hero = TavernHero::new(runtime);

        // Register a minimal agent
        let config = AgentConfig {
            id: "test-deepseek".into(),
            name: "DeepSeek Test".into(),
            description: None,
            model: ModelConfig {
                provider: "deepseek".into(),
                name: "deepseek-chat".into(),
                temperature: 0.3,
            },
            instructions: "You are a helpful assistant. Answer concisely.".into(),
            skills: vec![],
            constraints: vec![],
            memory: MemoryConfig::default(),
        };
        hero.register_agent(config).await.unwrap();

        // Execute — this calls real DeepSeek API via agent-core
        let result = hero.execute("test-deepseek", "What is 2+2? Reply with just the number.", None).await;

        assert!(result.is_ok(), "Real LLM call failed: {:?}", result.err());
        let value = result.unwrap();
        let text = value.as_str().unwrap();
        assert!(text.contains("4"), "Expected '4' in response, got: {}", text);
        eprintln!("Real DeepSeek response: {}", text);
    }
}
