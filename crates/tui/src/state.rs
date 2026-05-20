use crate::client::model::{ApiError, SessionInfo};
use std::collections::HashMap;
use std::time::SystemTime;
use ratatui::text::Line;

pub type SessionId = String;

#[derive(Debug)]
pub struct State {
    pub sessions: HashMap<SessionId, SessionState>,
    pub active_session: SessionId,
    pub connection_status: ConnectionStatus,
    pub last_error: Option<String>,
}

impl State {
    pub fn new(session_id: SessionId, info: SessionInfo) -> Self {
        let mut sessions = HashMap::new();
        sessions.insert(session_id.clone(), SessionState::new(info));
        Self { sessions, active_session: session_id, connection_status: ConnectionStatus::Connected, last_error: None }
    }
    pub fn active_session(&self) -> &SessionState {
        self.sessions.get(&self.active_session).expect("active session must exist")
    }
    pub fn active_session_mut(&mut self) -> &mut SessionState {
        self.sessions.get_mut(&self.active_session).expect("active session must exist")
    }
}

#[derive(Debug)]
pub struct SessionState {
    pub info: SessionInfo,
    pub messages: Vec<RenderedMessage>,
    pub streaming: Option<StreamingBuffer>,
    pub error: Option<ApiError>,
}

impl SessionState {
    pub fn new(info: SessionInfo) -> Self {
        Self { info, messages: Vec::new(), streaming: None, error: None }
    }
}

#[derive(Debug, Clone)]
pub struct RenderedMessage {
    pub role: MessageRole,
    pub blocks: Vec<MessageBlock>,
    pub timestamp: SystemTime,
    pub status: MessageStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole { User, Assistant }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageStatus { Streaming, Complete, Aborted, Error }

#[derive(Debug, Clone)]
pub enum MessageBlock {
    Text(Vec<Line<'static>>),
    ToolCall(ToolCallWidget),
    Thinking(ThinkingBlock),
    BashExecution(BashExecutionBlock),
    CompactionSummary(CompactionSummaryBlock),
}

#[derive(Debug, Clone)]
pub struct ToolCallWidget {
    pub call_id: String,
    pub name: String,
    pub state: ToolCallState,
    pub content: Vec<Line<'static>>,
    pub is_expanded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallState { Pending, Success, Error }

#[derive(Debug, Clone)]
pub struct ThinkingBlock {
    pub thinking_text: String,
    pub is_expanded: bool,
    pub is_redacted: bool,
}

#[derive(Debug, Clone)]
pub struct BashExecutionBlock {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub expanded: bool,
}

#[derive(Debug, Clone)]
pub struct CompactionSummaryBlock {
    pub summary: String,
    pub tokens_before: Option<u64>,
    pub tokens_after: Option<u64>,
    pub expanded: bool,
}

#[derive(Debug)]
pub struct StreamingBuffer {
    pub text_content: String,
    pub thinking_content: String,
    pub pending_tool_calls: Vec<ToolCallWidget>,
    pub tool_arg_buffers: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus { Disconnected, Connected, Reconnecting }

#[cfg(test)]
mod tests {
    use super::*;
    fn make_session_info(id: &str) -> SessionInfo {
        SessionInfo { id: id.to_string(), title: None, model: "gpt-4o".to_string(), context_window: Some(200000), created_at: None }
    }
    #[test]
    fn test_state_creation() {
        let info = make_session_info("s1");
        let state = State::new("s1".to_string(), info);
        assert_eq!(state.active_session, "s1");
    }
    #[test]
    fn test_message_status_values() {
        assert_ne!(MessageStatus::Streaming, MessageStatus::Complete);
        assert_ne!(MessageStatus::Aborted, MessageStatus::Error);
    }
    #[test]
    fn test_tool_call_state_values() {
        assert_ne!(ToolCallState::Pending, ToolCallState::Success);
    }
    #[test]
    fn test_streaming_buffer_tool_arg_accumulation() {
        let mut buf = StreamingBuffer { text_content: String::new(), thinking_content: String::new(), pending_tool_calls: Vec::new(), tool_arg_buffers: HashMap::new() };
        buf.tool_arg_buffers.insert("c1".to_string(), r#"{"path":"/tmp/foo"}"#.to_string());
        assert!(buf.tool_arg_buffers.contains_key("c1"));
    }
}
