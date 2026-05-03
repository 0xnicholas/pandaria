use agent_core::types::{CompactionEntry, SessionEntry, SessionEntryKind};
use agent_core::AgentMessage;

#[test]
fn test_session_entry_message_serde() {
    let entry = SessionEntry {
        id: 42,
        kind: SessionEntryKind::Message(AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text {
                text: "hello".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::UNIX_EPOCH,
        })),
    };

    let json = serde_json::to_string(&entry).unwrap();
    assert!(json.contains("\"id\":42"));
    assert!(json.contains("message"));

    let deserialized: SessionEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.id, 42);
    assert!(matches!(deserialized.kind, SessionEntryKind::Message(_)));
}

#[test]
fn test_session_entry_compaction_serde() {
    let entry = SessionEntry {
        id: 100,
        kind: SessionEntryKind::Compaction(CompactionEntry {
            summary: "compacted context".to_string(),
            first_kept_entry_id: 50,
            tokens_before: 1000,
            timestamp: std::time::SystemTime::UNIX_EPOCH,
            details: Some(serde_json::json!({"key": "value"})),
        }),
    };

    let json = serde_json::to_string(&entry).unwrap();
    assert!(json.contains("\"id\":100"));
    assert!(json.contains("compaction"));
    assert!(json.contains("compacted context"));

    let deserialized: SessionEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.id, 100);
    match deserialized.kind {
        SessionEntryKind::Compaction(compaction) => {
            assert_eq!(compaction.summary, "compacted context");
            assert_eq!(compaction.first_kept_entry_id, 50);
            assert_eq!(compaction.tokens_before, 1000);
            assert_eq!(compaction.details.unwrap()["key"], "value");
        }
        _ => panic!("expected compaction"),
    }
}

#[test]
fn test_session_entry_array_serde() {
    let entries = vec![
        SessionEntry {
            id: 1,
            kind: SessionEntryKind::Message(AgentMessage::User(llm_client::UserMessage {
                content: vec![llm_client::Content::Text {
                    text: "first".to_string(),
                    text_signature: None,
                }],
                timestamp: std::time::SystemTime::UNIX_EPOCH,
            })),
        },
        SessionEntry {
            id: 2,
            kind: SessionEntryKind::Compaction(CompactionEntry {
                summary: "summary".to_string(),
                first_kept_entry_id: 1,
                tokens_before: 500,
                timestamp: std::time::SystemTime::UNIX_EPOCH,
                details: None,
            }),
        },
        SessionEntry {
            id: 3,
            kind: SessionEntryKind::Message(AgentMessage::Assistant(llm_client::AssistantMessage {
                content: vec![llm_client::Content::Text {
                    text: "response".to_string(),
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
                timestamp: std::time::SystemTime::UNIX_EPOCH,
            })),
        },
    ];

    let json = serde_json::to_string(&entries).unwrap();
    let deserialized: Vec<SessionEntry> = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.len(), 3);
    assert_eq!(deserialized[0].id, 1);
    assert_eq!(deserialized[1].id, 2);
    assert_eq!(deserialized[2].id, 3);
}

#[test]
fn test_message_clone_equality() {
    let msg = AgentMessage::User(llm_client::UserMessage {
        content: vec![llm_client::Content::Text {
            text: "test".to_string(),
            text_signature: None,
        }],
        timestamp: std::time::SystemTime::UNIX_EPOCH,
    });

    let cloned = msg.clone();
    assert_eq!(msg, cloned);
}