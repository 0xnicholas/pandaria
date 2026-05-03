use crate::types::{Api, AssistantMessage, Content, Message, StopReason, Usage};

/// Options controlling message transformation behavior.
#[derive(Debug, Clone, Default)]
pub struct TransformOptions {
    /// Target API protocol identifier (e.g., "anthropic-messages").
    pub target_api: Option<String>,
    /// Whether the target model supports image input.
    pub supports_images: bool,
    /// Whether to preserve thinking blocks (same-model cross-turn only).
    pub preserve_thinking: bool,
}

/// Transform message list for cross-provider compatibility.
///
/// Applies four transformations in order:
/// 1. Image downgrade (§25.4)
/// 2. Thinking block handling (§25.5)
/// 3. Tool call ID normalization (§25.3)
/// 4. Orphan tool call padding (§25.6)
pub fn transform_messages(messages: &[Message], options: &TransformOptions) -> Vec<Message> {
    let mut result: Vec<Message> = messages.to_vec();

    // 1. Image downgrade
    if !options.supports_images {
        downgrade_images(&mut result);
    }

    // 2. Thinking block handling
    if !options.preserve_thinking {
        remove_thinking_blocks(&mut result);
    }

    // 3. Tool call ID normalization
    normalize_tool_call_ids(&mut result);

    // 4. Orphan tool call padding
    pad_orphan_tool_results(&mut result);

    result
}

fn downgrade_images(messages: &mut [Message]) {
    for msg in messages.iter_mut() {
        let (content, is_tool_result) = match msg {
            Message::User(m) => (&mut m.content, false),
            Message::ToolResult(m) => (&mut m.content, true),
            _ => continue,
        };

        let placeholder = if is_tool_result {
            "(tool image omitted: model does not support images)"
        } else {
            "(image omitted: model does not support images)"
        };

        let mut new_content: Vec<Content> = Vec::new();
        let mut prev_was_placeholder = false;

        for c in content.drain(..) {
            match c {
                Content::Image { .. } => {
                    if !prev_was_placeholder {
                        new_content.push(Content::Text {
                            text: placeholder.to_string(),
                            text_signature: None,
                        });
                        prev_was_placeholder = true;
                    }
                }
                other => {
                    prev_was_placeholder = false;
                    new_content.push(other);
                }
            }
        }
        *content = new_content;
    }
}

fn remove_thinking_blocks(messages: &mut [Message]) {
    for msg in messages.iter_mut() {
        if let Message::Assistant(m) = msg {
            m.content.retain(|c| !matches!(c, Content::Thinking { .. }));
        }
    }
}

fn normalize_tool_call_ids(messages: &mut [Message]) {
    // First pass: normalize IDs in assistant messages, build rename map
    let mut id_map: Vec<(String, String)> = Vec::new();

    for msg in messages.iter_mut() {
        if let Message::Assistant(m) = msg {
            for c in &mut m.content {
                if let Content::ToolCall(tc) = c {
                    let normalized = normalize_id(&tc.id);
                    if normalized != tc.id {
                        id_map.push((tc.id.clone(), normalized.clone()));
                        tc.id = normalized;
                    }
                }
            }
        }
    }

    // Second pass: update corresponding tool results
    for msg in messages.iter_mut() {
        if let Message::ToolResult(m) = msg {
            for (old_id, new_id) in &id_map {
                if &m.tool_call_id == old_id {
                    m.tool_call_id = new_id.clone();
                }
            }
        }
    }
}

/// Normalize a single tool call ID.
/// IDs longer than 64 characters are truncated with a short hash suffix.
fn normalize_id(id: &str) -> String {
    if id.len() <= 64 {
        return id.to_string();
    }
    let hash = short_hash(id);
    format!("call_{}{}", hash, &id[id.len().saturating_sub(8)..])
}

fn pad_orphan_tool_results(messages: &mut Vec<Message>) {
    let mut result: Vec<Message> = Vec::with_capacity(messages.len() + 4);
    let mut prev_was_assistant = false;

    for msg in messages.drain(..) {
        if matches!(msg, Message::ToolResult(_)) && !prev_was_assistant {
            result.push(Message::Assistant(AssistantMessage {
                content: vec![],
                provider: "system".to_string(),
                api: Api {
                    provider: "transform".to_string(),
                    model: "".to_string(),
                },
                model: "".to_string(),
                usage: Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: StopReason::ToolUse,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            }));
        }
        prev_was_assistant = matches!(msg, Message::Assistant(_));
        result.push(msg);
    }
    *messages = result;
}

pub(crate) fn short_hash(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:08x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolResultMessage;

    fn make_tool_call(id: &str) -> Content {
        Content::ToolCall(crate::ToolCall {
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

    #[test]
    fn test_tool_call_id_truncation() {
        let long_id = "a".repeat(100);
        let normalized = normalize_id(&long_id);
        assert!(normalized.len() <= 64);
        assert_ne!(normalized, long_id);
    }

    #[test]
    fn test_tool_call_id_short_preserved() {
        let short = "call_123".to_string();
        assert_eq!(normalize_id(&short), short);
    }

    #[test]
    fn test_tool_call_id_preserves_mapping() {
        let long_id = "a".repeat(100);
        let tc = make_tool_call(&long_id);
        let messages = vec![
            Message::Assistant(AssistantMessage {
                content: vec![tc.clone()],
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
                stop_reason: StopReason::ToolUse,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            }),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: long_id.clone(),
                tool_name: "test".into(),
                content: vec![],
                details: None,
                is_error: false,
                timestamp: std::time::SystemTime::now(),
            }),
        ];
        let result = transform_messages(
            &messages,
            &TransformOptions {
                preserve_thinking: true,
                ..Default::default()
            },
        );
        // Tool result ID should match the normalized tool call ID
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
        let messages = vec![Message::User(crate::UserMessage {
            content: vec![
                Content::Text {
                    text: "look at this".into(),
                    text_signature: None,
                },
                make_image(),
                make_image(),
            ],
            timestamp: std::time::SystemTime::now(),
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
        let messages = vec![Message::User(crate::UserMessage {
            content: vec![make_image()],
            timestamp: std::time::SystemTime::now(),
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
        let messages = vec![Message::Assistant(AssistantMessage {
            content: vec![
                Content::Thinking {
                    thinking: "hmm".into(),
                    thinking_signature: None,
                    redacted: false,
                },
                Content::Text {
                    text: "answer".into(),
                    text_signature: None,
                },
            ],
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
            timestamp: std::time::SystemTime::now(),
        })];
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
        let messages = vec![Message::Assistant(AssistantMessage {
            content: vec![Content::Thinking {
                thinking: "hmm".into(),
                thinking_signature: None,
                redacted: false,
            }],
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
            timestamp: std::time::SystemTime::now(),
        })];
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
}
