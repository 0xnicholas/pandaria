//! Query builder for memory retrieval — builds a search query from the most
//! recent user messages. Fact extraction is handled by the external memory
//! system (Emerald), not by Pandaria.

use crate::types::AgentMessage;

/// Build a retrieval query string from the most recent 1–3 user messages.
pub fn build_query_string(messages: &[AgentMessage]) -> String {
    messages
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
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_provider::{Content, UserMessage};

    #[test]
    fn test_build_query_empty() {
        let query = build_query_string(&[]);
        assert!(query.is_empty());
    }

    #[test]
    fn test_build_query_from_user_messages() {
        let messages = vec![AgentMessage::User(UserMessage {
            content: vec![Content::Text {
                text: "What is Rust?".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })];
        let query = build_query_string(&messages);
        assert!(query.contains("What is Rust?"));
    }
}
