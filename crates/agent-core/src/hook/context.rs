use crate::types::AgentMessage;

#[derive(Debug, Clone)]
pub enum CompactReason {
    Overflow,
    Threshold,
    Manual,
}

/// Context passed to Extension::on_tool_call
#[derive(Debug, Clone)]
pub struct ToolCallCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub input: serde_json::Value,
}

/// Context passed to Extension::on_tool_result
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

/// Context passed to Extension::on_turn_end
#[derive(Debug, Clone)]
pub struct TurnEndCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub turn_index: u64,
    pub messages: Vec<AgentMessage>,
    pub usage: ai_provider::Usage,
}

/// Context passed to Extension::on_agent_end
#[derive(Debug, Clone)]
pub struct AgentEndCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub messages: Vec<AgentMessage>,
}

/// Context passed to Extension::on_session_start
#[derive(Debug, Clone)]
pub struct SessionCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub system_prompt: String,
    pub tools: Vec<serde_json::Value>,
}

/// Context passed to Extension::on_context
#[derive(Debug, Clone)]
pub struct ContextCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub messages: Vec<AgentMessage>,
}

/// Context passed to Extension::on_before_agent_start
#[derive(Debug, Clone)]
pub struct BeforeAgentStartCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub system_prompt: Option<String>,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<serde_json::Value>,
    pub model: String,
}

/// Context passed to Extension::on_before_provider_request
#[derive(Debug, Clone)]
pub struct ProviderRequestCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub model: String,
    pub system_prompt: Option<String>,
    pub messages: Vec<AgentMessage>,
    pub turn_index: u64,
    pub tools: Option<Vec<ai_provider::ToolDef>>,
    pub options: crate::utils::provider_opts::ProviderStreamOptions,
}

/// Context passed to Extension::on_after_provider_response
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

/// Context passed to Extension::on_before_compact
#[derive(Debug, Clone)]
pub struct CompactCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub preparation: crate::compaction::CompactionPreparation,
    pub entries: Vec<crate::persistence::entry::SessionEntry>,
    pub reason: CompactReason,
}

/// Context passed to Extension::on_tool_execution_start
#[derive(Debug, Clone)]
pub struct ToolExecutionStartCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub input: serde_json::Value,
}

/// Context passed to Extension::on_tool_execution_end
#[derive(Debug, Clone)]
pub struct ToolExecutionEndCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub success: bool,
}

/// Context passed to Extension::on_compact_end
#[derive(Debug, Clone)]
pub struct CompactEndCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub compacted_messages: Vec<AgentMessage>,
    pub token_savings: usize,
}
