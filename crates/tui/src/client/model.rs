use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: Option<String>,
    pub model: String,
    pub context_window: Option<u64>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ServerEvent {
    #[serde(rename = "message_start")]
    MessageStart { message_index: u64 },
    #[serde(rename = "text_delta")]
    TextDelta { delta: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { content_index: usize, delta: String },
    #[serde(rename = "tool_call_started")]
    ToolCallStarted { call_id: String, name: String },
    #[serde(rename = "tool_call_delta")]
    ToolCallDelta { call_id: String, delta: String },
    #[serde(rename = "tool_call_done")]
    ToolCallDone { call_id: String, result: Option<String>, #[serde(default)] is_error: bool },
    #[serde(rename = "turn_end")]
    TurnEnd { stop_reason: String, usage: Option<UsageInfo> },
    #[serde(rename = "error")]
    Error { code: String, message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageInfo {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest { pub content: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest { pub title: Option<String> }

/// Historical message returned by `GET /sessions/{id}/messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum HistoricalMessage {
    #[serde(rename = "user")]
    User(HistoricalUserMessage),
    #[serde(rename = "assistant")]
    Assistant(HistoricalAssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(HistoricalToolResultMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalUserMessage {
    pub content: Vec<HistoricalContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalAssistantMessage {
    pub content: Vec<HistoricalContent>,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<HistoricalContent>,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HistoricalContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "toolCall")]
    ToolCall { id: String, name: String, arguments: serde_json::Value },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_delta_serde() {
        let json = r#"{"type":"text_delta","delta":"hello"}"#;
        let event: ServerEvent = serde_json::from_str(json).unwrap();
        match event {
            ServerEvent::TextDelta { delta } => assert_eq!(delta, "hello"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[test]
    fn test_tool_call_started_serde() {
        let json = r#"{"type":"tool_call_started","call_id":"c1","name":"read"}"#;
        let event: ServerEvent = serde_json::from_str(json).unwrap();
        match event {
            ServerEvent::ToolCallStarted { call_id, name } => {
                assert_eq!(call_id, "c1"); assert_eq!(name, "read");
            }
            _ => panic!("expected ToolCallStarted"),
        }
    }

    #[test]
    fn test_turn_end_serde() {
        let json = r#"{"type":"turn_end","stop_reason":"stop","usage":{"input_tokens":100,"output_tokens":50}}"#;
        let event: ServerEvent = serde_json::from_str(json).unwrap();
        match event {
            ServerEvent::TurnEnd { stop_reason, usage } => {
                assert_eq!(stop_reason, "stop");
                assert_eq!(usage.unwrap().input_tokens, 100);
            }
            _ => panic!("expected TurnEnd"),
        }
    }

    #[test]
    fn test_error_serde() {
        let json = r#"{"type":"error","code":"rate_limited","message":"too fast"}"#;
        let event: ServerEvent = serde_json::from_str(json).unwrap();
        match event {
            ServerEvent::Error { code, message } => {
                assert_eq!(code, "rate_limited"); assert_eq!(message, "too fast");
            }
            _ => panic!("expected Error"),
        }
    }
}
