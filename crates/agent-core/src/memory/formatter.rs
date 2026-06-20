//! Conversation formatter — produces Markdown transcripts and structured metadata
//! for external memory systems (Emerald, etc.) to consume.
//!
//! Pandaria does NOT do fact extraction itself. The external memory system
//! handles extraction, chunking, embedding, and relationship inference.
//! Pandaria's job is to prepare well-formatted raw data.

use std::time::SystemTime;

use ai_provider::{Content, Usage};
use crate::types::AgentMessage;

/// Summary of a tool call for metadata purposes (name + outcome, no full params).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TurnToolCallSummary {
    pub name: String,
    pub is_error: bool,
    pub result_len: usize,
}

/// Format a turn's messages as Markdown for external memory system consumption.
///
/// Output example:
/// ```markdown
/// ## Turn 3
///
/// **User**: 帮我重构 src/main.rs
///
/// **ToolCall[read_file]**: (see ToolResult below)
///
/// **ToolResult[read_file]**: (成功, 120 字符)
/// fn main() { println!("hello"); }
///
/// **Assistant**: 我已经重构了 main.rs，主要改动有...
/// ```
pub fn format_turn_content(turn_index: u64, messages: &[AgentMessage]) -> String {
    let mut out = String::new();
    out.push_str(&format!("## Turn {}\n\n", turn_index));

    for msg in messages {
        match msg {
            AgentMessage::User(u) => {
                let text = collect_text(&u.content);
                if !text.is_empty() {
                    out.push_str(&format!("**User**: {}\n\n", text));
                }
            }
            AgentMessage::Assistant(a) => {
                let tool_call_names: Vec<&str> = a
                    .content
                    .iter()
                    .filter_map(|c| {
                        if let Content::ToolCall(tc) = c {
                            Some(tc.name.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();

                if !tool_call_names.is_empty() {
                    for name in &tool_call_names {
                        out.push_str(&format!(
                            "**ToolCall[{}]**: (see ToolResult below)\n\n",
                            name
                        ));
                    }
                }

                let text = a
                    .content
                    .iter()
                    .filter_map(|c| {
                        if let Content::Text { text, .. } = c {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                if !text.is_empty() {
                    out.push_str(&format!("**Assistant**: {}\n\n", text));
                }
            }
            AgentMessage::ToolResult(tr) => {
                let text = collect_text(&tr.content);
                let status = if tr.is_error { "失败" } else { "成功" };
                let summary = truncate_text(&text, 500);
                out.push_str(&format!(
                    "**ToolResult[{}]**: ({}, {} 字符)\n{}\n\n",
                    tr.tool_name,
                    status,
                    text.len(),
                    summary,
                ));
            }
        }
    }

    out
}

/// Build structured metadata for a turn, for external memory system indexing.
#[allow(clippy::too_many_arguments)]
pub fn build_turn_metadata(
    tenant_id: &str,
    session_id: &str,
    turn_index: u64,
    model: &str,
    usage: &Usage,
    stop_reason: &str,
    tool_calls: &[TurnToolCallSummary],
    timestamp: SystemTime,
) -> serde_json::Value {
    serde_json::json!({
        "tenant_id": tenant_id,
        "session_id": session_id,
        "turn_index": turn_index,
        "model": model,
        "stop_reason": stop_reason,
        "token_usage": {
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "total_tokens": usage.total_tokens,
        },
        "tool_calls": tool_calls,
        "timestamp": format!("{:?}", timestamp),
    })
}

/// Extract tool call summaries from messages for metadata.
pub fn extract_tool_summaries(messages: &[AgentMessage]) -> Vec<TurnToolCallSummary> {
    let mut summaries = Vec::new();
    let mut tool_call_names: Vec<String> = Vec::new();

    for msg in messages {
        if let AgentMessage::Assistant(a) = msg {
            for c in &a.content {
                if let Content::ToolCall(tc) = &c {
                    tool_call_names.push(tc.name.clone());
                }
            }
        }
        if let AgentMessage::ToolResult(tr) = msg {
            let name = tool_call_names
                .first()
                .cloned()
                .unwrap_or_else(|| tr.tool_name.clone());
            if !tool_call_names.is_empty() {
                tool_call_names.remove(0);
            }
            let text = collect_text(&tr.content);
            summaries.push(TurnToolCallSummary {
                name,
                is_error: tr.is_error,
                result_len: text.len(),
            });
        }
    }

    summaries
}

// ── helpers ──

fn collect_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| {
            if let Content::Text { text, .. } = c {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}...(截断, 共 {} 字符)", &text[..max_len], text.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_provider::{
        AssistantMessage, Content, ToolCall, ToolResultMessage, Usage, UserMessage,
    };

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: vec![Content::Text {
                text: text.to_string(),
                text_signature: None,
            }],
            timestamp: SystemTime::now(),
        })
    }

    fn assistant_text(text: &str) -> AgentMessage {
        AgentMessage::Assistant(AssistantMessage {
            content: vec![Content::Text {
                text: text.to_string(),
                text_signature: None,
            }],
            provider: "test".into(),
            model: "test".into(),
            api: ai_provider::Api {
                provider: "test".into(),
                model: "test".into(),
            },
            usage: Usage {
                input_tokens: 0, output_tokens: 0, total_tokens: 0,
                cache_creation_input_tokens: None, cache_read_input_tokens: None,
            },
            stop_reason: ai_provider::StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: SystemTime::now(),
        })
    }

    fn assistant_tool_call(name: &str, args: &str) -> AgentMessage {
        AgentMessage::Assistant(AssistantMessage {
            content: vec![Content::ToolCall(ToolCall {
                id: "tc1".into(),
                name: name.to_string(),
                arguments: serde_json::from_str(args).unwrap(),
                thought_signature: None,
            })],
            provider: "test".into(),
            model: "test".into(),
            api: ai_provider::Api {
                provider: "test".into(),
                model: "test".into(),
            },
            usage: Usage {
                input_tokens: 0, output_tokens: 0, total_tokens: 0,
                cache_creation_input_tokens: None, cache_read_input_tokens: None,
            },
            stop_reason: ai_provider::StopReason::ToolUse,
            response_id: None,
            error_message: None,
            timestamp: SystemTime::now(),
        })
    }

    fn tool_result(name: &str, text: &str, is_error: bool) -> AgentMessage {
        AgentMessage::ToolResult(ToolResultMessage {
            tool_name: name.to_string(),
            tool_call_id: "tc1".into(),
            content: vec![Content::Text {
                text: text.to_string(),
                text_signature: None,
            }],
            is_error,
            details: None,
            timestamp: SystemTime::now(),
        })
    }

    #[test]
    fn test_format_simple_turn() {
        let messages = vec![user_msg("hello"), assistant_text("hi there")];
        let output = format_turn_content(1, &messages);
        assert!(output.contains("## Turn 1"));
        assert!(output.contains("**User**: hello"));
        assert!(output.contains("**Assistant**: hi there"));
    }

    #[test]
    fn test_format_turn_with_tool_calls() {
        let messages = vec![
            user_msg("read src/main.rs"),
            assistant_tool_call("read_file", r#"{"path":"src/main.rs"}"#),
            tool_result("read_file", "fn main() { println!(\"hello\"); }", false),
            assistant_text("I found the main function."),
        ];
        let output = format_turn_content(2, &messages);
        assert!(output.contains("## Turn 2"));
        assert!(output.contains("**ToolCall[read_file]**"));
        assert!(output.contains("**ToolResult[read_file]**"));
        assert!(output.contains("(成功"));
    }

    #[test]
    fn test_format_turn_tool_error() {
        let messages = vec![
            user_msg("delete /etc/passwd"),
            assistant_tool_call("delete_file", r#"{"path":"/etc/passwd"}"#),
            tool_result("delete_file", "Permission denied", true),
        ];
        let output = format_turn_content(3, &messages);
        assert!(output.contains("(失败"));
    }

    #[test]
    fn test_build_turn_metadata() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        };
        let tools = vec![TurnToolCallSummary {
            name: "read_file".into(),
            is_error: false,
            result_len: 42,
        }];
        let metadata = build_turn_metadata(
            "t1", "s1", 1, "gpt-4", &usage, "stop", &tools,
            SystemTime::now(),
        );
        let m = metadata.as_object().unwrap();
        assert_eq!(m["tenant_id"], "t1");
        assert_eq!(m["session_id"], "s1");
        assert_eq!(m["turn_index"], 1);
        assert_eq!(m["model"], "gpt-4");
        assert_eq!(m["stop_reason"], "stop");
        assert_eq!(m["token_usage"]["input_tokens"], 100);
        let tc = m["tool_calls"].as_array().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0]["name"], "read_file");
    }

    #[test]
    fn test_format_empty_turn() {
        let output = format_turn_content(0, &[]);
        assert!(output.contains("## Turn 0"));
    }

    #[test]
    fn test_extract_tool_summaries() {
        let messages = vec![
            user_msg("run test"),
            assistant_tool_call("bash", r#"{"command":"cargo test"}"#),
            tool_result("bash", "test result: ok. 5 passed", false),
        ];
        let summaries = extract_tool_summaries(&messages);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].name, "bash");
        assert!(!summaries[0].is_error);
        assert!(summaries[0].result_len > 0);
    }
}
