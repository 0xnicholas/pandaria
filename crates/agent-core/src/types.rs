/// Type alias for the LLM message type used throughout the agent system.
pub type AgentMessage = ai_provider::Message;

// Re-export session entry types from session_entry module
pub use crate::persistence::entry::{CompactionDetails, SessionContextBuilder, SessionEntry};

// Re-export tool types from the tools module (backward-compatible path)
pub use crate::tools::{
    AgentTool, AgentToolProgressUpdate, AgentToolRef, AgentToolResult, ToolExecutionMode,
};
