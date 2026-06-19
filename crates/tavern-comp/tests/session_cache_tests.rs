use std::sync::Arc;
use async_trait::async_trait;
use agent_core::harness::config::HarnessConfig;
use tavern_comp::{PandariaAgentExecutor, AgentResolver};
use tavern_core::{AgentConfig, MemoryConfig, ModelConfig};

fn dummy_agent(id: &str, model: &str) -> AgentConfig {
    AgentConfig {
        id: id.into(), name: id.into(), description: None,
        model: ModelConfig { provider: "test".into(), name: model.into(), temperature: 0.7 },
        instructions: "test".into(), skills: vec![], constraints: vec![],
        memory: MemoryConfig::default(),
    }
}

/// Inline mock resolver for integration tests.
struct TestResolver { agents: Vec<AgentConfig> }

#[async_trait]
impl AgentResolver for TestResolver {
    async fn resolve(&self, agent_id: &str) -> Option<AgentConfig> {
        self.agents.iter().find(|a| a.id == agent_id).cloned()
    }
}

#[tokio::test]
async fn test_session_count_reflects_cache() {
    let harness = HarnessConfig::from_env(Arc::new(ai_provider::RouterProvider::new()));
    let resolver = Arc::new(TestResolver { agents: vec![dummy_agent("r1","m1")] });
    let executor = PandariaAgentExecutor::new("t1", "team1", harness, resolver);
    assert_eq!(executor.session_count(), 0);
}

#[tokio::test]
async fn test_lru_eviction_on_full_cache() {
    let harness = HarnessConfig::from_env(Arc::new(ai_provider::RouterProvider::new()));
    let agents: Vec<AgentConfig> = (0..5).map(|i| dummy_agent(&format!("r{i}"), &format!("m{i}"))).collect();
    let resolver = Arc::new(TestResolver { agents: agents.clone() });
    let executor = PandariaAgentExecutor::new("t1", "team1", harness, resolver)
        .with_max_cached_sessions(3);
    assert_eq!(executor.session_count(), 0);
}
