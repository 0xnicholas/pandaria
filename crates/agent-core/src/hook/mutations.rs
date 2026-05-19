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
    pub content: Option<Vec<ai_provider::Content>>,
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

use crate::prompt::{PromptBuilder, PromptMutation};

/// Mutation returned by on_before_agent_start hook
#[derive(Debug, Clone, Default)]
pub struct BeforeAgentStartMutation {
    /// Legacy field: wholesale replacement of the prompt builder.
    /// When set, skills are automatically re-injected afterwards.
    pub system_prompt: Option<PromptBuilder>,
    /// Surgical prompt mutation applied to the current builder.
    /// Skills are preserved by default (mutate, not replace).
    pub prompt_mutation: Option<PromptMutation>,
    pub messages: Option<Vec<AgentMessage>>,
}

/// Mutation returned by on_before_provider_request hook
#[derive(Debug, Clone, Default)]
pub struct ProviderRequestMutation {
    /// Legacy field: wholesale replacement of the prompt builder.
    /// When set, skills are automatically re-injected afterwards.
    pub system_prompt: Option<PromptBuilder>,
    /// Surgical prompt mutation applied to the current builder.
    /// Skills are preserved by default (mutate, not replace).
    pub prompt_mutation: Option<PromptMutation>,
    pub messages: Option<Vec<AgentMessage>>,
    pub tools: Option<Option<Vec<ai_provider::ToolDef>>>,
    pub options: Option<crate::utils::provider_opts::ProviderStreamOptions>,
}

/// Mutation returned by on_after_provider_response hook
#[derive(Debug, Clone, Default)]
pub struct ProviderResponseMutation {
    pub content: Option<Vec<ai_provider::Content>>,
    pub stop_reason: Option<ai_provider::StopReason>,
}
