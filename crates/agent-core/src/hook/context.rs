use crate::prompt::PromptBuilder;
use crate::types::AgentMessage;

#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum CompactReason {
    Overflow,
    Threshold,
    Manual,
}

/// Context passed to Extension::on_tool_call
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ToolCallCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub input: serde_json::Value,
}

impl ToolCallCtx {
    /// Create a new `ToolCallCtx` with the given identifiers.
    ///
    /// `input` defaults to `Value::Null`; assign directly after construction if needed.
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            tool_name: tool_name.into(),
            tool_call_id: tool_call_id.into(),
            input: serde_json::Value::Null,
        }
    }
}

/// Context passed to Extension::on_tool_result
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ToolResultCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub input: serde_json::Value,
    pub content: Vec<ai_provider::Content>,
    pub details: Option<serde_json::Value>,
    pub is_error: bool,
}

impl ToolResultCtx {
    /// Create a new `ToolResultCtx` with the given identifiers.
    ///
    /// `content` defaults to empty, `details` to `None`, `is_error` to `false`.
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            tool_name: tool_name.into(),
            tool_call_id: tool_call_id.into(),
            input: serde_json::Value::Null,
            content: vec![],
            details: None,
            is_error: false,
        }
    }
}

/// Context passed to Extension::on_turn_end
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct TurnEndCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub turn_index: u64,
    pub messages: Vec<AgentMessage>,
    pub usage: ai_provider::Usage,
}

impl TurnEndCtx {
    /// Create a new `TurnEndCtx` with the given identifiers, turn index and usage.
    ///
    /// `messages` defaults to empty; assign directly after construction if needed.
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        turn_index: u64,
        usage: ai_provider::Usage,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            turn_index,
            messages: vec![],
            usage,
        }
    }
}

/// Context passed to Extension::on_agent_end
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct AgentEndCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub messages: Vec<AgentMessage>,
}

impl AgentEndCtx {
    /// Create a new `AgentEndCtx` with the given identifiers.
    ///
    /// `messages` defaults to empty; assign directly after construction if needed.
    pub fn new(tenant_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            messages: vec![],
        }
    }
}

/// Context passed to Extension::on_session_start
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct SessionCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub system_prompt: String,
    pub tools: Vec<serde_json::Value>,
}

impl SessionCtx {
    /// Create a new `SessionCtx` with the given identifiers.
    ///
    /// `system_prompt` defaults to empty, `tools` to empty.
    pub fn new(tenant_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            system_prompt: String::new(),
            tools: vec![],
        }
    }
}

/// Context passed to Extension::on_context
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ContextCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub messages: Vec<AgentMessage>,
}

impl ContextCtx {
    /// Create a new `ContextCtx` with the given identifiers.
    ///
    /// `messages` defaults to empty; assign directly after construction if needed.
    pub fn new(tenant_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            messages: vec![],
        }
    }
}

/// Context passed to Extension::on_before_agent_start
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct BeforeAgentStartCtx {
    pub tenant_id: String,
    pub session_id: String,
    /// Rendered system prompt (legacy convenience field).
    pub system_prompt: Option<String>,
    /// The prompt builder that Extension may inspect or clone-and-modify.
    pub prompt_builder: PromptBuilder,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<serde_json::Value>,
    pub model: String,
}

impl BeforeAgentStartCtx {
    /// Create a new `BeforeAgentStartCtx` with the given identifiers and model.
    ///
    /// `system_prompt` defaults to `None`, `prompt_builder` to default,
    /// `messages` and `tools` to empty.
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            system_prompt: None,
            prompt_builder: PromptBuilder::default(),
            messages: vec![],
            tools: vec![],
            model: model.into(),
        }
    }
}

/// Context passed to Extension::on_before_provider_request
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ProviderRequestCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub model: String,
    /// Rendered system prompt (legacy convenience field).
    pub system_prompt: Option<String>,
    /// The prompt builder that Extension may inspect or clone-and-modify.
    pub prompt_builder: PromptBuilder,
    pub messages: Vec<AgentMessage>,
    pub turn_index: u64,
    pub tools: Option<Vec<ai_provider::ToolDef>>,
    pub options: crate::utils::provider_opts::ProviderStreamOptions,
}

impl ProviderRequestCtx {
    /// Create a new `ProviderRequestCtx` with the given identifiers, model and turn index.
    ///
    /// `system_prompt` defaults to `None`, `prompt_builder` to default,
    /// `messages` to empty, `tools` to `None`,
    /// `options` to `ProviderStreamOptions::default()`.
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        model: impl Into<String>,
        turn_index: u64,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            model: model.into(),
            system_prompt: None,
            prompt_builder: PromptBuilder::default(),
            messages: vec![],
            turn_index,
            tools: None,
            options: crate::utils::provider_opts::ProviderStreamOptions::default(),
        }
    }
}

/// Context passed to Extension::on_after_provider_response
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ProviderResponseCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub model: String,
    pub content: Vec<ai_provider::Content>,
    pub turn_index: u64,
    pub attempt: u32,
    pub messages_before: Vec<AgentMessage>,
    pub stop_reason: ai_provider::StopReason,
}

impl ProviderResponseCtx {
    /// Create a new `ProviderResponseCtx` with the given identifiers, model, turn index and stop reason.
    ///
    /// `content` and `messages_before` default to empty, `attempt` to `0`.
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        model: impl Into<String>,
        turn_index: u64,
        stop_reason: ai_provider::StopReason,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            model: model.into(),
            content: vec![],
            turn_index,
            attempt: 0,
            messages_before: vec![],
            stop_reason,
        }
    }
}

/// Context passed to Extension::on_before_compact
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct CompactCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub preparation: crate::compaction::CompactionPreparation,
    pub entries: Vec<crate::persistence::entry::SessionEntry>,
    pub reason: CompactReason,
}

impl CompactCtx {
    /// Create a new `CompactCtx` with the given identifiers, preparation and reason.
    ///
    /// `entries` defaults to empty; assign directly after construction if needed.
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        preparation: crate::compaction::CompactionPreparation,
        reason: CompactReason,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            preparation,
            entries: vec![],
            reason,
        }
    }
}

/// Context passed to Extension::on_tool_execution_start
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ToolExecutionStartCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub input: serde_json::Value,
}

impl ToolExecutionStartCtx {
    /// Create a new `ToolExecutionStartCtx` with the given identifiers.
    ///
    /// `input` defaults to `Value::Null`; assign directly after construction if needed.
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            tool_name: tool_name.into(),
            tool_call_id: tool_call_id.into(),
            input: serde_json::Value::Null,
        }
    }
}

/// Context passed to Extension::on_tool_execution_end
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ToolExecutionEndCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub success: bool,
}

impl ToolExecutionEndCtx {
    /// Create a new `ToolExecutionEndCtx` with the given identifiers.
    ///
    /// `success` defaults to `false`; assign directly after construction if needed.
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            tool_name: tool_name.into(),
            tool_call_id: tool_call_id.into(),
            success: false,
        }
    }
}

/// Context passed to Extension::on_compact_end
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct CompactEndCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub compacted_messages: Vec<AgentMessage>,
    pub token_savings: usize,
    /// The compaction result, if compaction succeeded.
    pub result: Option<crate::compaction::CompactionResult>,
}

impl CompactEndCtx {
    /// Create a new `CompactEndCtx` with the given identifiers.
    ///
    /// `compacted_messages` defaults to empty, `token_savings` to `0`,
    /// `result` to `None`.
    pub fn new(tenant_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            compacted_messages: vec![],
            token_savings: 0,
            result: None,
        }
    }
}
