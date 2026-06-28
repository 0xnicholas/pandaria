use std::path::Path;

use async_trait::async_trait;
use crate::team::executor::AgentResolver;
use tavern_core::{AgentConfig, AgentSummary};
use tokio::sync::RwLock;
use tracing::{info, instrument};

use super::error::TavernError;
use super::registry::AgentRegistry;

/// Agent 管理核心，负责加载、注册和查询 Agent 配置。
/// SquadEngine 通过 AgentResolver trait 查找 Agent 配置。
pub struct TavernHero {
    registry: RwLock<AgentRegistry>,
}

impl TavernHero {
    pub fn new() -> Self {
        Self {
            registry: RwLock::new(AgentRegistry::new()),
        }
    }

    /// 从目录批量加载 YAML 配置。
    #[instrument(skip(self))]
    pub async fn load_from_dir(&self, dir: &Path) -> Result<(), TavernError> {
        let configs = super::loader::load_from_dir(dir)?;
        let mut registry = self.registry.write().await;
        for (config, path) in configs {
            registry.register(config).map_err(|e| e.with_path(&path))?;
        }
        let count = registry.len();
        drop(registry);
        info!(count = count, "loaded agents from directory");
        Ok(())
    }

    /// V0.3.8: 运行时注册 Agent（不依赖 YAML 文件）。
    pub async fn register_agent(&self, config: AgentConfig) -> Result<(), TavernError> {
        let id = config.id.clone();
        let mut registry = self.registry.write().await;
        registry.register(config)?;
        drop(registry);
        info!(agent_id = %id, "agent registered at runtime");
        Ok(())
    }

    /// V0.3.8: 运行时移除 Agent。
    /// 若 id 不存在返回 AgentNotFound。
    pub async fn unregister_agent(&self, id: &str) -> Result<(), TavernError> {
        let mut registry = self.registry.write().await;
        registry
            .unregister(id)
            .ok_or_else(|| TavernError::AgentNotFound { id: id.to_string() })?;
        drop(registry);
        info!(agent_id = %id, "agent unregistered at runtime");
        Ok(())
    }

    /// 热重载：清空后从目录重新加载所有 Agent。
    #[instrument(skip(self))]
    pub async fn reload_from_dir(&self, dir: &Path) -> Result<(), TavernError> {
        let configs = super::loader::load_from_dir(dir)?;
        let mut registry = self.registry.write().await;
        registry.clear();
        for (config, path) in configs {
            if let Err(e) = registry.register(config) {
                tracing::warn!("failed to register agent from {:?}: {}", path, e);
            }
        }
        let count = registry.len();
        drop(registry);
        info!(count = count, "agents hot reloaded");
        Ok(())
    }

    /// 加载单个 Agent 配置，返回注册的 agent_id。
    #[instrument(skip(self))]
    pub async fn load_agent(&self, path: &Path) -> Result<String, TavernError> {
        let config = super::loader::load_agent(path)?;
        let id = config.id.clone();
        let mut registry = self.registry.write().await;
        registry.register(config).map_err(|e| e.with_path(path))?;
        drop(registry);
        info!(agent_id = %id, "loaded agent from file");
        Ok(id)
    }

    /// 查询已注册 Agent。
    pub async fn get_agent(&self, id: &str) -> Option<AgentConfig> {
        self.registry.read().await.get(id).cloned()
    }

    /// 列出所有已注册 Agent（返回完整配置的克隆）。
    pub async fn list_agents(&self) -> Vec<AgentConfig> {
        self.registry
            .read()
            .await
            .list_all()
            .into_iter()
            .cloned()
            .collect()
    }

    /// 列出所有已注册 Agent 的摘要。
    pub async fn list_agents_summary(&self) -> Vec<AgentSummary> {
        self.registry.read().await.list_summary()
    }
}

#[async_trait]
impl AgentResolver for TavernHero {
    async fn resolve(&self, agent_id: &str) -> Option<AgentConfig> {
        self.get_agent(agent_id).await
    }
}
