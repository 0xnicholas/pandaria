use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Content {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text_signature: Option<String>,
    },
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking_signature: Option<String>,
        #[serde(default)]
        redacted: bool,
    },
    #[serde(rename = "toolCall")]
    ToolCall(ToolCall),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserMessage {
    pub content: Vec<Content>,
    #[serde(with = "ts_serde")]
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantMessage {
    pub content: Vec<Content>,
    pub provider: String,
    pub api: Api,
    pub model: String,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(with = "ts_serde")]
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(with = "ts_serde")]
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Api {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: u64,
}

impl Usage {
    pub fn compute_total(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_input_tokens.unwrap_or(0)
            + self.cache_read_input_tokens.unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role")]
pub enum Message {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(ToolResultMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

mod ts_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(time: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        let dur = time
            .duration_since(UNIX_EPOCH)
            .expect("SystemTime before UNIX_EPOCH — clock is incorrect");
        dur.as_secs().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let secs: u64 = Deserialize::deserialize(d)?;
        Ok(UNIX_EPOCH + Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    #[test]
    fn test_user_message_json_roundtrip() {
        let msg = UserMessage {
            content: vec![Content::Text {
                text: "hello".to_string(),
                text_signature: None,
            }],
            timestamp: SystemTime::UNIX_EPOCH,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: UserMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content.len(), 1);
    }

    #[test]
    fn test_assistant_message_json_roundtrip() {
        let msg = AssistantMessage {
            content: vec![
                Content::Text {
                    text: "ok".to_string(),
                    text_signature: None,
                },
                Content::ToolCall(ToolCall {
                    id: "c1".to_string(),
                    name: "read".to_string(),
                    arguments: serde_json::json!({"path": "/x"}),
                    thought_signature: None,
                }),
            ],
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            api: Api {
                provider: "openai".to_string(),
                model: "gpt-4".to_string(),
            },
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 15,
            },
            stop_reason: StopReason::ToolUse,
            response_id: None,
            error_message: None,
            timestamp: SystemTime::UNIX_EPOCH,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: AssistantMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stop_reason, StopReason::ToolUse);
        assert_eq!(back.content.len(), 2);
        assert_eq!(back.provider, "openai");
        assert_eq!(back.model, "gpt-4");
    }

    #[test]
    fn test_message_enum_tagged_serialization() {
        let msg = Message::ToolResult(ToolResultMessage {
            tool_call_id: "c1".to_string(),
            tool_name: "read".to_string(),
            content: vec![Content::Text {
                text: "ok".to_string(),
                text_signature: None,
            }],
            details: None,
            is_error: false,
            timestamp: SystemTime::UNIX_EPOCH,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"toolResult\""));
        let back: Message = serde_json::from_str(&json).unwrap();
        match back {
            Message::ToolResult(m) => assert_eq!(m.tool_name, "read"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_stop_reason_serde() {
        let sr = StopReason::ToolUse;
        let json = serde_json::to_string(&sr).unwrap();
        assert_eq!(json, "\"tool_use\"");
        let back: StopReason = serde_json::from_str(&json).unwrap();
        assert_eq!(back, StopReason::ToolUse);
    }

    #[test]
    fn test_thinking_content_serde() {
        let content = Content::Thinking {
            thinking: "hmm".into(),
            thinking_signature: Some("sig123".into()),
            redacted: false,
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("\"thinking\""));
        assert!(json.contains("\"thinking_signature\":\"sig123\""));
        let back: Content = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(back, Content::Thinking { ref thinking, thinking_signature: Some(ref sig), redacted } if thinking == "hmm" && sig == "sig123" && !redacted)
        );
    }

    #[test]
    fn test_thinking_content_serde_redacted() {
        let content = Content::Thinking {
            thinking: "hmm".into(),
            thinking_signature: None,
            redacted: true,
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("\"redacted\":true"));
        let back: Content = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Content::Thinking { redacted, .. } if redacted));
    }

    #[test]
    fn test_toolcall_signature_serde() {
        let tc = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: serde_json::json!({}),
            thought_signature: Some("ts123".into()),
        };
        let json = serde_json::to_string(&tc).unwrap();
        assert!(json.contains("\"thought_signature\":\"ts123\""));
        let back: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.thought_signature.unwrap(), "ts123");
    }
}
