use serde::{Deserialize, Serialize};

/// SSE 事件类型，与 TUI 客户端 `client/model.rs` 保持字段级兼容。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ServerEvent {
    #[serde(rename = "message_start")]
    MessageStart { message_index: u64 },

    #[serde(rename = "text_delta")]
    TextDelta { delta: String },

    /// 协议预留：当前 agent-core 未生成对应事件，MVP 不会触发。
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { content_index: usize, delta: String },

    /// 协议预留：当前 agent-core 未生成对应事件，MVP 不会触发。
    #[serde(rename = "tool_call_started")]
    ToolCallStarted { call_id: String, name: String },

    /// 协议预留：当前 agent-core 未生成对应事件，MVP 不会触发。
    #[serde(rename = "tool_call_delta")]
    ToolCallDelta { call_id: String, delta: String },

    #[serde(rename = "tool_call_done")]
    ToolCallDone {
        call_id: String,
        result: Option<String>,
        #[serde(default)]
        is_error: bool,
    },

    #[serde(rename = "turn_end")]
    TurnEnd {
        stop_reason: String,
        usage: Option<UsageInfo>,
    },

    #[serde(rename = "error")]
    Error { code: String, message: String },
}

/// Token 使用量统计。api-gateway 独立定义，不依赖 ai-provider 的 Usage 类型。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageInfo {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

/// Session 元数据（gateway 视角）。
/// 与 tenant crate 的 `SessionInfo` 结构对齐，但 id 序列化为 String 以匹配 TUI。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: Option<String>,
    pub model: String,
    pub context_window: Option<u64>,
    pub created_at: Option<String>,
}

impl From<tenant::SessionInfo> for SessionInfo {
    fn from(info: tenant::SessionInfo) -> Self {
        Self {
            id: info.id.to_string(),
            title: info.title,
            model: info.model,
            context_window: None, // 由 gateway handler 从 ServerConfig 填充
            created_at: Some(info.created_at),
        }
    }
}

/// 创建 session 请求体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub title: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
}

/// 消息内容片段，支持多模态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContentPart {
    Text { text: String },
    Image { data: String, mime_type: String },
    Video { data: String, mime_type: String },
    Audio { data: String, mime_type: String },
}

/// 发送消息请求体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub content: Vec<MessageContentPart>,
}

/// 更新 session 请求体（所有字段可选）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateSessionRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
}

/// API 统一错误响应体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ApiError,
}

/// 发送消息成功响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub turn_index: u64,
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;
    use super::*;

    #[test]
    fn test_server_event_serde_roundtrip() {
        let events = vec![
            ServerEvent::MessageStart { message_index: 0 },
            ServerEvent::TextDelta { delta: "hello".into() },
            ServerEvent::TurnEnd {
                stop_reason: "stop".into(),
                usage: Some(UsageInfo {
                    input_tokens: 10,
                    output_tokens: 5,
                }),
            },
            ServerEvent::Error {
                code: "rate_limited".into(),
                message: "too fast".into(),
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let back: ServerEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, back);
        }
    }

    #[test]
    fn test_create_session_request_missing_system_prompt() {
        let json = r#"{"title": null}"#;
        let req: CreateSessionRequest = serde_json::from_str(json).unwrap();
        assert!(req.title.is_none());
        assert!(req.system_prompt.is_none());
    }

    #[test]
    fn test_create_session_request_with_system_prompt() {
        let json = r#"{"title": "test", "system_prompt": "you are a helpful assistant"}"#;
        let req: CreateSessionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.title, Some("test".into()));
        assert_eq!(req.system_prompt, Some("you are a helpful assistant".into()));
    }

    #[test]
    fn test_session_info_from_tenant() {
        let tenant_info = tenant::SessionInfo {
            id: Uuid::new_v4(),
            tenant_id: "t1".into(),
            created_at: "1234567890".into(),
            turn_count: 0,
            system_prompt: None,
            title: Some("test".into()),
            model: "claude".into(),
        };
        let info: SessionInfo = tenant_info.into();
        assert_eq!(info.title, Some("test".into()));
        assert_eq!(info.model, "claude");
        assert_eq!(info.context_window, None);
    }
}
