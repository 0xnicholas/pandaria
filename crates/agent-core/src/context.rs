use crate::types::AgentMessage;

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
    pub content: Vec<llm_client::Content>,
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
