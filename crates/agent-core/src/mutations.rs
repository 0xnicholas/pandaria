use crate::types::AgentMessage;

/// Decision returned by first-block-wins hooks
#[derive(Debug, Clone, Default)]
pub enum HookDecision {
    #[default]
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

/// Mutation returned by blocking hooks for tool calls.
/// Supports input parameter modification.
#[derive(Debug, Clone, Default)]
pub struct ToolCallMutation {
    pub input: Option<serde_json::Value>,
}

/// Decision returned by on_before_compact hook
#[derive(Debug, Clone, Default)]
pub enum CompactDecision {
    #[default]
    Continue,
    Block { reason: String },
    Replace { result: crate::compaction::CompactionResult },
}

/// Mutation returned by on_before_agent_start hook
#[derive(Debug, Clone, Default)]
pub struct BeforeAgentStartMutation {
    pub system_prompt: Option<String>,
    pub messages: Option<Vec<AgentMessage>>,
}

/// Mutation returned by on_before_provider_request hook
#[derive(Debug, Clone, Default)]
pub struct ProviderRequestMutation {
    pub system_prompt: Option<Option<String>>,
    pub messages: Option<Vec<AgentMessage>>,
    pub tools: Option<Option<Vec<llm_client::ToolDef>>>,
    pub options: Option<crate::provider_opts::ProviderStreamOptions>,
}

/// Mutation returned by on_after_provider_response hook
#[derive(Debug, Clone, Default)]
pub struct ProviderResponseMutation {
    pub content: Option<Vec<llm_client::Content>>,
    pub stop_reason: Option<llm_client::StopReason>,
}
