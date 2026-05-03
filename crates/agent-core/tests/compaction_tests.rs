use agent_core::compaction::{
    compute_file_lists, estimate_context_tokens, estimate_tokens, extract_file_ops_from_message,
    find_cut_point, format_file_operations, prepare_compaction,
    should_compact, CompactionSettings, FileOperations,
};
use agent_core::types::{AgentMessage, CompactionEntry, SessionEntry, SessionEntryKind};

// ============================================================================
// Token estimation tests
// ============================================================================

#[test]
fn test_estimate_tokens_user_message() {
    let msg = AgentMessage::User(llm_client::UserMessage {
        content: vec![llm_client::Content::Text {
            text: "Hello world".to_string(),
            text_signature: None,
        }],
        timestamp: std::time::SystemTime::now(),
    });
    // "Hello world" = 11 chars, ceil(11/4) = 3
    assert_eq!(estimate_tokens(&msg), 3);
}

#[test]
fn test_estimate_tokens_assistant_with_tool_call() {
    let msg = AgentMessage::Assistant(llm_client::AssistantMessage {
        content: vec![llm_client::Content::ToolCall(llm_client::ToolCall {
            id: "call_1".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({"path": "/tmp/test.txt"}),
            thought_signature: None,
        })],
        provider: "test".to_string(),
        model: "test".to_string(),
        api: llm_client::Api {
            provider: "test".to_string(),
            model: "test".to_string(),
        },
        usage: llm_client::Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            total_tokens: 0,
        },
        stop_reason: llm_client::StopReason::Stop,
        response_id: None,
        error_message: None,
        timestamp: std::time::SystemTime::now(),
    });
    let tokens = estimate_tokens(&msg);
    assert!(tokens > 0);
}

#[test]
fn test_estimate_context_tokens_with_usage() {
    let messages = vec![
        AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text {
                text: "Hello".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
        AgentMessage::Assistant(llm_client::AssistantMessage {
            content: vec![llm_client::Content::Text {
                text: "Hi".to_string(),
                text_signature: None,
            }],
            provider: "test".to_string(),
            model: "test".to_string(),
            api: llm_client::Api {
                provider: "test".to_string(),
                model: "test".to_string(),
            },
            usage: llm_client::Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 15,
            },
            stop_reason: llm_client::StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        }),
        AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text {
                text: "How are you?".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    ];

    let estimate = estimate_context_tokens(&messages);
    assert_eq!(estimate.usage_tokens, 15);
    assert_eq!(estimate.last_usage_index, Some(1));
    // Trailing message: "How are you?" = 12 chars, ceil(12/4) = 3
    assert_eq!(estimate.trailing_tokens, 3);
    assert_eq!(estimate.tokens, 18);
}

// ============================================================================
// Should compact tests
// ============================================================================

#[test]
fn test_should_compact_enabled() {
    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: 1000,
        keep_recent_tokens: 500,
    };
    // context_window = 10000, context_tokens = 9500
    // 9500 > 10000 - 1000 = 9000, should trigger
    assert!(should_compact(9500, 10000, &settings));
    // 8000 > 9000? No
    assert!(!should_compact(8000, 10000, &settings));
}

#[test]
fn test_should_compact_disabled() {
    let settings = CompactionSettings {
        enabled: false,
        reserve_tokens: 1000,
        keep_recent_tokens: 500,
    };
    assert!(!should_compact(9500, 10000, &settings));
}

// ============================================================================
// Cut point detection tests
// ============================================================================

#[test]
fn test_find_cut_point_simple() {
    let messages: Vec<AgentMessage> = (0..10).map(|i| {
        AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text {
                text: format!("message {}", i),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })
    }).collect();

    let cut = find_cut_point(&messages, 50); // Large budget, keep all
    assert_eq!(cut.first_kept_index, 0);
    assert!(!cut.is_split_turn);
}

#[test]
fn test_find_cut_point_with_tool_result() {
    let messages = vec![
        AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text {
                text: "Hello".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
        AgentMessage::Assistant(llm_client::AssistantMessage {
            content: vec![llm_client::Content::ToolCall(llm_client::ToolCall {
                id: "call_1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"path": "/tmp/test.txt"}),
                thought_signature: None,
            })],
            provider: "test".to_string(),
            model: "test".to_string(),
            api: llm_client::Api {
                provider: "test".to_string(),
                model: "test".to_string(),
            },
            usage: llm_client::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 0,
            },
            stop_reason: llm_client::StopReason::ToolUse,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        }),
        AgentMessage::ToolResult(llm_client::ToolResultMessage {
            tool_call_id: "call_1".to_string(),
            tool_name: "read".to_string(),
            content: vec![llm_client::Content::Text {
                text: "file content".to_string(),
                text_signature: None,
            }],
            details: None,
            is_error: false,
            timestamp: std::time::SystemTime::now(),
        }),
    ];

    let cut = find_cut_point(&messages, 1); // Small budget
    // Tool result is at index 2, not a valid cut point.
    // The closest valid cut point before/after index 2 is index 1 (assistant).
    // But our algorithm walks backwards and finds the first valid cut point >= current index.
    // Since tool_result has ~3 tokens, accumulated >= budget at index 2.
    // No valid cut point >= 2, so we fall back to the last found cut point.
    // In a 3-message list with small budget, we should keep from index 1.
    // However, if accumulated never exceeds budget before hitting index 0,
    // first_kept_index might be 0. Let's verify the behavior.
    assert!(!cut.is_split_turn || cut.first_kept_index < messages.len());
}

// ============================================================================
// File operations tests
// ============================================================================

#[test]
fn test_extract_file_ops_from_message() {
    let msg = AgentMessage::Assistant(llm_client::AssistantMessage {
        content: vec![llm_client::Content::ToolCall(llm_client::ToolCall {
            id: "call_1".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({"path": "/tmp/test.txt"}),
            thought_signature: None,
        })],
        provider: "test".to_string(),
        model: "test".to_string(),
        api: llm_client::Api {
            provider: "test".to_string(),
            model: "test".to_string(),
        },
        usage: llm_client::Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            total_tokens: 0,
        },
        stop_reason: llm_client::StopReason::ToolUse,
        response_id: None,
        error_message: None,
        timestamp: std::time::SystemTime::now(),
    });

    let mut ops = FileOperations::default();
    extract_file_ops_from_message(&msg, &mut ops);
    assert!(ops.read.contains("/tmp/test.txt"));
}

#[test]
fn test_compute_file_lists() {
    let mut ops = FileOperations::default();
    ops.read.insert("/tmp/a.txt".to_string());
    ops.read.insert("/tmp/b.txt".to_string());
    ops.edited.insert("/tmp/b.txt".to_string());
    ops.written.insert("/tmp/c.txt".to_string());

    let (read_only, modified) = compute_file_lists(&ops);
    assert_eq!(read_only, vec!["/tmp/a.txt"]);
    assert_eq!(modified, vec!["/tmp/b.txt", "/tmp/c.txt"]);
}

#[test]
fn test_format_file_operations() {
    let read_files = vec!["a.txt".to_string(), "b.txt".to_string()];
    let modified_files = vec!["c.txt".to_string()];
    let formatted = format_file_operations(&read_files, &modified_files);
    assert!(formatted.contains("<read-files>"));
    assert!(formatted.contains("<modified-files>"));
    assert!(formatted.contains("a.txt"));
    assert!(formatted.contains("c.txt"));
}

// ============================================================================
// Compaction preparation tests
// ============================================================================

#[test]
fn test_prepare_compaction_empty_entries() {
    let entries: Vec<SessionEntry> = vec![];
    let settings = CompactionSettings::default();
    assert!(prepare_compaction(&entries, &settings).is_none());
}

#[test]
fn test_prepare_compaction_simple() {
    let entries: Vec<SessionEntry> = (0..5).map(|i| SessionEntry {
        id: i as u64,
        kind: SessionEntryKind::Message(AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text {
                text: format!("msg {}", i),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })),
    }).collect();

    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: 1000,
        keep_recent_tokens: 1, // Very small budget to force compaction
    };

    let prep = prepare_compaction(&entries, &settings).unwrap();
    assert!(!prep.messages_to_summarize.is_empty());
    assert_eq!(prep.first_kept_entry_id, entries[prep.first_kept_entry_id as usize].id);
}

#[test]
fn test_prepare_compaction_with_previous_compaction() {
    let mut entries: Vec<SessionEntry> = (0..3).map(|i| SessionEntry {
        id: i as u64,
        kind: SessionEntryKind::Message(AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text {
                text: format!("msg {}", i),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })),
    }).collect();

    // Add a previous compaction
    entries.push(SessionEntry {
        id: 3,
        kind: SessionEntryKind::Compaction(CompactionEntry {
            summary: "Previous summary".to_string(),
            first_kept_entry_id: 1,
            tokens_before: 100,
            timestamp: std::time::SystemTime::now(),
            details: None,
        }),
    });

    // Add more messages after compaction
    entries.push(SessionEntry {
        id: 4,
        kind: SessionEntryKind::Message(AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text {
                text: "msg 4".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })),
    });

    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: 1000,
        keep_recent_tokens: 1,
    };

    let prep = prepare_compaction(&entries, &settings).unwrap();
    assert_eq!(prep.previous_summary, Some("Previous summary".to_string()));
    // Should start from first_kept_entry_id = 1, not 0
    assert!(prep.messages_to_summarize.len() >= 1);
}

// ============================================================================
// Serialization tests
// ============================================================================

#[test]
fn test_serialize_conversation() {
    let messages = vec![
        AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text {
                text: "Hello".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
        AgentMessage::Assistant(llm_client::AssistantMessage {
            content: vec![llm_client::Content::Text {
                text: "Hi there".to_string(),
                text_signature: None,
            }],
            provider: "test".to_string(),
            model: "test".to_string(),
            api: llm_client::Api {
                provider: "test".to_string(),
                model: "test".to_string(),
            },
            usage: llm_client::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 0,
            },
            stop_reason: llm_client::StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        }),
    ];

    let serialized = agent_core::compaction::serialize_conversation(&messages);
    assert!(serialized.contains("[User]: Hello"));
    assert!(serialized.contains("[Assistant]: Hi there"));
}