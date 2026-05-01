use async_trait::async_trait;
use std::sync::Arc;

use crate::error::AgentError;

pub type AgentMessage = llm_client::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolExecutionMode {
    #[default]
    Sequential,
    Parallel,
}

#[derive(Debug, Clone)]
pub struct AgentToolResult {
    pub content: Vec<llm_client::Content>,
    pub details: Option<serde_json::Value>,
    pub is_error: bool,
}

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;

    fn description(&self) -> &str;

    fn parameters(&self) -> serde_json::Value;

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Sequential
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError>;
}

pub type AgentToolRef = Arc<dyn AgentTool>;
