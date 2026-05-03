use crate::types::AgentMessage;

/// Decision returned by first-block-wins hooks
#[derive(Debug, Clone)]
pub enum HookDecision {
    Continue,
    Block { reason: String },
}

/// Mutation returned by chain hooks for tool results
#[derive(Debug, Clone, Default)]
pub struct ToolResultMutation {
    pub content: Option<Vec<llm_client::Content>>,
    pub details: Option<serde_json::Value>,
    pub is_error: Option<bool>,
    /// Override the terminate flag set by the tool execution.
    /// When set, replaces the tool's terminate flag.
    pub terminate: Option<bool>,
}

/// Mutation returned by chain hooks for context
#[derive(Debug, Clone, Default)]
pub struct ContextMutation {
    pub messages: Option<Vec<AgentMessage>>,
}
