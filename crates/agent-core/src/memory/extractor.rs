use crate::types::AgentMessage;

use super::types::{MemoryFact, MemoryQuery};

/// Extract key facts from a single turn's messages.
///
/// MVP strategy:
/// - Assistant final replies (text content only, no tool calls) → importance 5
/// - Important tool results (non-error, length > 10) → importance 4
/// - Skip user raw input and error messages.
pub fn extract_facts(messages: &[AgentMessage]) -> Vec<MemoryFact> {
    let mut facts = Vec::new();
    for msg in messages {
        match msg {
            AgentMessage::Assistant(a) => {
                let text: String = a
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ai_provider::Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() && text.len() > 20 {
                    facts.push(MemoryFact {
                        id: None,
                        content: text,
                        category: Some("assistant_response".to_string()),
                        importance: Some(5),
                        metadata: serde_json::Value::Null,
                    });
                }
            }
            AgentMessage::ToolResult(tr) if !tr.is_error => {
                let text: String = tr
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ai_provider::Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() && text.len() > 10 {
                    facts.push(MemoryFact {
                        id: None,
                        content: format!("[Tool: {}] {}", tr.tool_name, text),
                        category: Some("tool_result".to_string()),
                        importance: Some(4),
                        metadata: serde_json::json!({"tool_name": tr.tool_name}),
                    });
                }
            }
            _ => {}
        }
    }
    facts
}

/// Build a retrieval query from the most recent 1–3 user messages.
pub fn build_query(messages: &[AgentMessage]) -> MemoryQuery {
    let recent_user_text: Vec<String> = messages
        .iter()
        .rev()
        .take(3)
        .filter_map(|m| {
            if let AgentMessage::User(u) = m {
                Some(
                    u.content
                        .iter()
                        .filter_map(|c| match c {
                            ai_provider::Content::Text { text, .. } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            } else {
                None
            }
        })
        .collect();

    MemoryQuery {
        text: recent_user_text.join("\n"),
        limit: 5,
        session_only: false,
    }
}

/// Format retrieved facts for injection into the LLM context.
pub fn format_facts(facts: &[MemoryFact]) -> String {
    facts
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n---\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_provider::{AssistantMessage, Content, ToolResultMessage, Usage, UserMessage};

    fn text_msg(text: impl Into<String>) -> AgentMessage {
        AgentMessage::Assistant(AssistantMessage {
            content: vec![Content::Text {
                text: text.into(),
                text_signature: None,
            }],
            provider: "test".to_string(),
            api: ai_provider::Api {
                provider: "test".to_string(),
                model: "test".to_string(),
            },
            model: "test".to_string(),
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 0,
            },
            stop_reason: ai_provider::StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        })
    }

    fn tool_result(name: impl Into<String>, text: impl Into<String>) -> AgentMessage {
        AgentMessage::ToolResult(ToolResultMessage {
            tool_name: name.into(),
            tool_call_id: "tc1".to_string(),
            content: vec![Content::Text {
                text: text.into(),
                text_signature: None,
            }],
            is_error: false,
            details: None,
            timestamp: std::time::SystemTime::now(),
        })
    }

    #[test]
    fn test_extract_facts_filters_short_text() {
        let messages = vec![text_msg("hi")];
        let facts = extract_facts(&messages);
        assert!(facts.is_empty());
    }

    #[test]
    fn test_extract_facts_extracts_assistant() {
        let messages = vec![text_msg(
            "This is a longer assistant reply that should be remembered.",
        )];
        let facts = extract_facts(&messages);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].category, Some("assistant_response".to_string()));
    }

    #[test]
    fn test_extract_facts_extracts_tool_result() {
        let messages = vec![tool_result("echo", "hello world")];
        let facts = extract_facts(&messages);
        assert_eq!(facts.len(), 1);
        assert!(facts[0].content.contains("echo"));
    }

    #[test]
    fn test_build_query_from_user_messages() {
        let messages = vec![
            AgentMessage::User(UserMessage {
                content: vec![Content::Text {
                    text: "What is Rust?".to_string(),
                    text_signature: None,
                }],
                timestamp: std::time::SystemTime::now(),
            }),
            text_msg("A systems programming language."),
        ];
        let query = build_query(&messages);
        assert!(query.text.contains("What is Rust?"));
        assert_eq!(query.limit, 5);
    }
}
