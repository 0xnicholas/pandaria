use llm_client::{
    Api, AssistantMessage, Content, Message, StopReason, ToolCall, ToolResultMessage, Usage,
    UserMessage,
};
use llm_client::transform::{transform_messages, TransformOptions};
use std::time::SystemTime;

fn make_tool_call(id: &str) -> Content {
    Content::ToolCall(ToolCall {
        id: id.to_string(),
        name: "test".to_string(),
        arguments: serde_json::json!({}),
        thought_signature: None,
    })
}

fn make_image() -> Content {
    Content::Image {
        data: "base64data".to_string(),
        mime_type: "image/png".to_string(),
    }
}

fn make_assistant(content: Vec<Content>) -> Message {
    Message::Assistant(AssistantMessage {
        content,
        provider: "test".into(),
        model: "test".into(),
        api: Api {
            provider: "test".into(),
            model: "test".into(),
        },
        usage: Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            total_tokens: 0,
        },
        stop_reason: StopReason::Stop,
        response_id: None,
        error_message: None,
        timestamp: SystemTime::now(),
    })
}

fn make_tool_result(tool_call_id: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: tool_call_id.to_string(),
        tool_name: "test".into(),
        content: vec![],
        details: None,
        is_error: false,
        timestamp: SystemTime::now(),
    })
}

#[test]
fn test_tool_call_id_truncation() {
    let long_id = "a".repeat(100);
    let messages = vec![make_assistant(vec![make_tool_call(&long_id)])];
    let result = transform_messages(
        &messages,
        &TransformOptions {
            preserve_thinking: true,
            ..Default::default()
        },
    );
    let assist = match &result[0] {
        Message::Assistant(m) => m,
        _ => panic!("expected assistant"),
    };
    let tc_id = match &assist.content[0] {
        Content::ToolCall(tc) => &tc.id,
        _ => panic!("expected tool call"),
    };
    assert!(tc_id.len() <= 64);
    assert_ne!(tc_id, &long_id);
}

#[test]
fn test_tool_call_id_short_preserved() {
    let short = "call_123";
    let messages = vec![make_assistant(vec![make_tool_call(short)])];
    let result = transform_messages(
        &messages,
        &TransformOptions {
            preserve_thinking: true,
            ..Default::default()
        },
    );
    let assist = match &result[0] {
        Message::Assistant(m) => m,
        _ => panic!("expected assistant"),
    };
    let tc_id = match &assist.content[0] {
        Content::ToolCall(tc) => &tc.id,
        _ => panic!("expected tool call"),
    };
    assert_eq!(tc_id, short);
}

#[test]
fn test_tool_call_id_preserves_mapping() {
    let long_id = "a".repeat(100);
    let messages = vec![
        make_assistant(vec![make_tool_call(&long_id)]),
        make_tool_result(&long_id),
    ];
    let result = transform_messages(
        &messages,
        &TransformOptions {
            preserve_thinking: true,
            ..Default::default()
        },
    );
    let assist = match &result[0] {
        Message::Assistant(m) => m,
        _ => panic!("expected assistant"),
    };
    let tc_id = match &assist.content[0] {
        Content::ToolCall(tc) => &tc.id,
        _ => panic!("expected tool call"),
    };
    let tool_result = match &result[1] {
        Message::ToolResult(m) => m,
        _ => panic!("expected tool result"),
    };
    assert_eq!(&tool_result.tool_call_id, tc_id);
}

#[test]
fn test_image_downgrade_non_vision() {
    let messages = vec![Message::User(UserMessage {
        content: vec![
            Content::Text {
                text: "look at this".into(),
                text_signature: None,
            },
            make_image(),
            make_image(),
        ],
        timestamp: SystemTime::now(),
    })];
    let result = transform_messages(
        &messages,
        &TransformOptions {
            preserve_thinking: true,
            ..Default::default()
        },
    );
    let user = match &result[0] {
        Message::User(m) => m,
        _ => panic!(),
    };
    // Should have text + one placeholder (consecutive images merged)
    assert_eq!(user.content.len(), 2);
    assert!(matches!(user.content[0], Content::Text { .. }));
    assert!(matches!(user.content[1], Content::Text { .. }));
}

#[test]
fn test_image_preserved_vision_model() {
    let messages = vec![Message::User(UserMessage {
        content: vec![make_image()],
        timestamp: SystemTime::now(),
    })];
    let result = transform_messages(
        &messages,
        &TransformOptions {
            supports_images: true,
            preserve_thinking: true,
            ..Default::default()
        },
    );
    let user = match &result[0] {
        Message::User(m) => m,
        _ => panic!(),
    };
    assert!(matches!(user.content[0], Content::Image { .. }));
}

#[test]
fn test_thinking_block_removed_cross_provider() {
    let messages = vec![make_assistant(vec![
        Content::Thinking {
            thinking: "hmm".into(),
            thinking_signature: None,
            redacted: false,
        },
        Content::Text {
            text: "answer".into(),
            text_signature: None,
        },
    ])];
    let result = transform_messages(
        &messages,
        &TransformOptions {
            preserve_thinking: false,
            ..Default::default()
        },
    );
    let assist = match &result[0] {
        Message::Assistant(m) => m,
        _ => panic!(),
    };
    assert_eq!(assist.content.len(), 1);
    assert!(matches!(assist.content[0], Content::Text { .. }));
}

#[test]
fn test_thinking_block_preserved_same_model() {
    let messages = vec![make_assistant(vec![Content::Thinking {
        thinking: "hmm".into(),
        thinking_signature: None,
        redacted: false,
    }])];
    let result = transform_messages(
        &messages,
        &TransformOptions {
            preserve_thinking: true,
            ..Default::default()
        },
    );
    let assist = match &result[0] {
        Message::Assistant(m) => m,
        _ => panic!(),
    };
    assert_eq!(assist.content.len(), 1);
    assert!(matches!(assist.content[0], Content::Thinking { .. }));
}

#[test]
fn test_orphan_tool_result_padding() {
    // ToolResult without preceding Assistant should get padded
    let messages = vec![make_tool_result("call_1")];
    let result = transform_messages(
        &messages,
        &TransformOptions {
            preserve_thinking: true,
            ..Default::default()
        },
    );
    assert_eq!(result.len(), 2);
    assert!(matches!(result[0], Message::Assistant(_)));
    assert!(matches!(result[1], Message::ToolResult(_)));
}
