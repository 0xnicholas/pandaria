use async_trait::async_trait;

use agent_core::types::{AgentTool, AgentToolProgressUpdate, AgentToolResult, ToolExecutionMode};
use agent_core::error::AgentError;

use super::extension_actor::ExtensionHandle;

/// Wraps an Extension-registered tool into an AgentTool that delegates
/// execution to the ExtensionActor via ExtensionHandle::execute_tool().
///
/// Multiple ExtensionTool instances may hold the same ExtensionHandle
/// (one per tool name registered by the same extension).
pub struct ExtensionTool {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) parameters: serde_json::Value,
    pub(crate) handle: ExtensionHandle,
    pub(crate) execution_mode: ToolExecutionMode,
}

#[async_trait]
impl AgentTool for ExtensionTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        self.execution_mode
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
    ) -> Result<AgentToolResult, AgentError> {
        // v0.1: no progress streaming for extension tools.
        self.handle.execute_tool(tool_call_id.to_string(), params).await
    }
}