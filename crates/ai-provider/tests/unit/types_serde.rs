use ai_provider::{
    Api, AssistantMessage, Content, Message, StopReason, ToolCall, ToolResultMessage, Usage,
    UserMessage,
};
use std::time::SystemTime;

#[test]
fn test_user_message_text_roundtrip() {
    let msg = UserMessage {
        content: vec![Content::Text {
            text: "hello world".to_string(),
            text_signature: None,
        }],
        timestamp: SystemTime::UNIX_EPOCH,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: UserMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back, msg);
}

#[test]
fn test_assistant_message_text_plus_toolcall_roundtrip() {
    let msg = AssistantMessage {
        content: vec![
            Content::Text {
                text: "ok".to_string(),
                text_signature: None,
            },
            Content::ToolCall(ToolCall {
                id: "call_123".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "/etc/passwd"}),
                thought_signature: None,
            }),
        ],
        provider: "anthropic".to_string(),
        api: Api {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
        },
        model: "claude-sonnet-4".to_string(),
        usage: Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            total_tokens: 150,
        },
        stop_reason: StopReason::ToolUse,
        response_id: Some("resp_abc".to_string()),
        error_message: None,
        timestamp: SystemTime::UNIX_EPOCH,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: AssistantMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back, msg);
}

#[test]
fn test_tool_result_message_with_error_flag_roundtrip() {
    let msg = ToolResultMessage {
        tool_call_id: "call_123".to_string(),
        tool_name: "read_file".to_string(),
        content: vec![Content::Text {
            text: "file not found".to_string(),
            text_signature: None,
        }],
        details: Some(serde_json::json!({"exit_code": 1})),
        is_error: true,
        timestamp: SystemTime::UNIX_EPOCH,
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"is_error\":true"));
    let back: ToolResultMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back, msg);
    assert!(back.is_error);
}

#[test]
fn test_message_enum_tagged_serialization() {
    let msg = Message::User(UserMessage {
        content: vec![Content::Text {
            text: "hi".to_string(),
            text_signature: None,
        }],
        timestamp: SystemTime::UNIX_EPOCH,
    });
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"role\":\"user\""));
    let back: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(back, msg);
}

#[test]
fn test_stop_reason_snake_case_serialization() {
    for (variant, expected) in [
        (StopReason::Stop, "\"stop\""),
        (StopReason::Length, "\"length\""),
        (StopReason::ToolUse, "\"tool_use\""),
        (StopReason::Error, "\"error\""),
        (StopReason::Aborted, "\"aborted\""),
    ] {
        let json = serde_json::to_string(&variant).unwrap();
        assert_eq!(json, expected);
        let back: StopReason = serde_json::from_str(&json).unwrap();
        assert_eq!(back, variant);
    }
}

#[test]
fn test_content_thinking_with_signature_roundtrip() {
    let content = Content::Thinking {
        thinking: "I need to think about this".to_string(),
        thinking_signature: Some("sig_abc123".to_string()),
        redacted: false,
    };
    let json = serde_json::to_string(&content).unwrap();
    assert!(json.contains("\"type\":\"thinking\""));
    assert!(json.contains("\"thinking_signature\":\"sig_abc123\""));
    let back: Content = serde_json::from_str(&json).unwrap();
    assert_eq!(back, content);
}

#[test]
fn test_content_thinking_redacted_roundtrip() {
    let content = Content::Thinking {
        thinking: "redacted".to_string(),
        thinking_signature: None,
        redacted: true,
    };
    let json = serde_json::to_string(&content).unwrap();
    assert!(json.contains("\"redacted\":true"));
    let back: Content = serde_json::from_str(&json).unwrap();
    assert_eq!(back, content);
    match back {
        Content::Thinking { redacted, .. } => assert!(redacted),
        _ => panic!("expected Thinking variant"),
    }
}

#[test]
fn test_tool_call_with_thought_signature_roundtrip() {
    let tc = ToolCall {
        id: "call_456".to_string(),
        name: "write_file".to_string(),
        arguments: serde_json::json!({"path": "/tmp/test", "content": "hello"}),
        thought_signature: Some("thought_sig_xyz".to_string()),
    };
    let json = serde_json::to_string(&tc).unwrap();
    assert!(json.contains("\"thought_signature\":\"thought_sig_xyz\""));
    let back: ToolCall = serde_json::from_str(&json).unwrap();
    assert_eq!(back, tc);
}

#[test]
fn test_api_roundtrip() {
    let api = Api {
        provider: "openai".to_string(),
        model: "gpt-5.2".to_string(),
    };
    let json = serde_json::to_string(&api).unwrap();
    let back: Api = serde_json::from_str(&json).unwrap();
    assert_eq!(back, api);
}

#[test]
fn test_usage_with_cache_tokens_roundtrip() {
    let usage = Usage {
        input_tokens: 1000,
        output_tokens: 500,
        cache_creation_input_tokens: Some(200),
        cache_read_input_tokens: Some(300),
        total_tokens: 2000,
    };
    let json = serde_json::to_string(&usage).unwrap();
    assert!(json.contains("\"cache_creation_input_tokens\":200"));
    assert!(json.contains("\"cache_read_input_tokens\":300"));
    let back: Usage = serde_json::from_str(&json).unwrap();
    assert_eq!(back, usage);
    assert_eq!(back.compute_total(), 2000);
}

#[test]
fn test_tool_def_serialization() {
    let tool = ai_provider::ToolDef {
        name: "test_tool".to_string(),
        description: "A test tool".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "x": {"type": "string"}
            }
        }),
    };
    let json = serde_json::to_string(&tool).unwrap();
    assert!(json.contains("\"name\":\"test_tool\""));
    assert!(json.contains("\"description\":\"A test tool\""));
    assert!(json.contains("\"parameters\""));
    let back: ai_provider::ToolDef = serde_json::from_str(&json).unwrap();
    assert_eq!(back.name, "test_tool");
    assert_eq!(back.description, "A test tool");
}

#[test]
fn test_content_image_variant() {
    let content = Content::Image {
        data: "base64data".to_string(),
        mime_type: "image/png".to_string(),
    };
    let json = serde_json::to_string(&content).unwrap();
    assert!(json.contains("\"type\":\"image\""));
    assert!(json.contains("\"data\":\"base64data\""));
    assert!(json.contains("\"mime_type\":\"image/png\""));
    let back: Content = serde_json::from_str(&json).unwrap();
    assert_eq!(back, content);
}
