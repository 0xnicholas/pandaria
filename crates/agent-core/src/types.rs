use async_trait::async_trait;
use std::sync::Arc;

use crate::error::AgentError;

/// Type alias for the LLM message type used throughout the agent system.
pub type AgentMessage = ai_provider::Message;

// Re-export session entry types from session_entry module
pub use crate::persistence::entry::{CompactionDetails, SessionContextBuilder, SessionEntry};

/// Controls how multiple tool calls within a single assistant response are executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolExecutionMode {
    /// Execute tool calls one at a time, in order.
    Sequential,
    /// Execute independent tool calls concurrently.
    #[default]
    Parallel,
}

/// Streaming progress update emitted during tool execution.
#[derive(Debug, Clone)]
pub struct AgentToolProgressUpdate {
    pub content: String,
}

/// Result of executing a single tool call.
#[derive(Debug, Clone)]
pub struct AgentToolResult {
    pub content: Vec<ai_provider::Content>,
    pub details: Option<serde_json::Value>,
    pub is_error: bool,
    /// When true, signals that the agent should stop after this tool result
    /// even if other tools are pending. Only takes effect if ALL tool results
    /// in a batch have terminate set to true.
    pub terminate: bool,
}

/// Trait for tools that can be called by the agent.
///
/// Implementations must be `Send + Sync` (interior mutability via `Mutex`/`RwLock`
/// if mutable state is needed).
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// Unique tool name as passed to the LLM.
    fn name(&self) -> &str;

    /// Human-readable description of what the tool does.
    fn description(&self) -> &str;

    /// JSON Schema describing the tool's parameters.
    fn parameters(&self) -> serde_json::Value;

    /// Override the default execution mode for this tool.
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Parallel
    }

    /// Execute the tool.
    ///
    /// Parameters:
    /// - `tool_call_id`: unique ID from the LLM assistant message
    /// - `params`: JSON arguments validated by the tool
    /// - `on_progress`: optional callback for streaming progress updates
    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
    ) -> Result<AgentToolResult, AgentError>;
}

/// Reference-counted owned pointer to a tool implementation.
pub type AgentToolRef = Arc<dyn AgentTool>;
