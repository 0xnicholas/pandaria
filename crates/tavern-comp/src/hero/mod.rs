pub mod error;
#[allow(clippy::module_inception)]
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

    #[tokio::test]
    async fn test_hero_resolve_agent_not_found() {
        use crate::team::executor::AgentResolver;
        let hero = TavernHero::new();
        let result = hero.resolve("unknown").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_hero_resolve_after_register() {
        use crate::team::executor::AgentResolver;
        let hero = TavernHero::new();
        let config = agent_with_id("test-agent");
        hero.register_agent(config).await.unwrap();
        let resolved = hero.resolve("test-agent").await;
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap().id, "test-agent");
    }
}
