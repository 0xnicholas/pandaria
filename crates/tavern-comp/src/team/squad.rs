use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use super::context::TeamContext;
use super::executor::AgentExecutor;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SquadStatus {
    Pending,
    Running,
    WaitingForSignal { signal: String },
    Sleeping { wake_at: DateTime<Utc> },
    Completed,
    Failed,
}

/// 一次 Agent Team 的执行实例。
///
/// 注意：`Squad` 不实现 `Serialize`，因为包含 `Arc<dyn AgentExecutor>`。
/// 持久化恢复时使用 `TeamContext` + `SquadStatus`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SquadResult {
    pub squad_id: String,
    pub team_id: String,
    pub status: SquadStatus,
    pub context: TeamContext,
    pub outputs: Value,
}

/// 一次 Agent Team 的执行实例。
///
/// 注意：`Squad` 不实现 `Serialize`，因为包含 `Arc<dyn AgentExecutor>`。
/// 持久化恢复时使用 `TeamContext` + `SquadStatus`。
#[derive(Clone)]
pub struct Squad {
    pub id: String,
    pub team_id: String,
    pub status: SquadStatus,
    pub context: TeamContext,
    pub executor: Arc<dyn AgentExecutor>,
    /// Signals received while the squad was waiting.
    pub received_signals: std::collections::HashSet<String>,
}

impl Squad {
    pub fn new(
        squad_id: String,
        team_id: String,
        executor: Arc<dyn AgentExecutor>,
    ) -> Self {
        Self {
            id: squad_id.clone(),
            team_id,
            status: SquadStatus::Pending,
            context: TeamContext::default(),
            executor,
            received_signals: std::collections::HashSet::new(),
        }
    }

    /// Record that a signal has been received, waking any mission waiting for it.
    pub fn send_signal(&mut self, signal: &str) {
        self.received_signals.insert(signal.to_string());
    }

    /// Check if a signal has been received and consume it.
    pub fn take_signal(&mut self, signal: &str) -> bool {
        self.received_signals.remove(signal)
    }
}
