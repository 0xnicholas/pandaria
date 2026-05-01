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
}

/// Mutation returned by chain hooks for context
#[derive(Debug, Clone, Default)]
pub struct ContextMutation {
    pub messages: Option<Vec<AgentMessage>>,
}
