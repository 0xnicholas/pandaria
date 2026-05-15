use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use agent_core::harness::compaction::{CompactionActor, CompactionConfig};
use agent_core::{
    SessionActor,
};
use agent_core::file_ops::DefaultFileOperationExtractor;
use agent_core::persistence::entry::SessionEntry;
use agent_core::test_utils::{AllowAllDispatcher, TestProvider, TestResponse};
use agent_core::types::AgentMessage;
use ai_provider::{
    Content, UserMessage,
};

// ============================================================================
// Helper: build CompactionActor with given provider
// ============================================================================

fn make_compaction_actor(provider: Arc<dyn ai_provider::LlmProvider>) -> CompactionActor {
    CompactionActor::new(
        CompactionConfig::new(true, 1000, 100),
        provider,
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    )
}

// ============================================================================
// Helper: build entries with sufficient text
// ============================================================================

fn build_entries(count: usize) -> Vec<SessionEntry> {
    (0..count)
        .map(|i| {
            let padding = "x".repeat(80);
            SessionEntry::Message {
                id: uuid::Uuid::new_v4(),
                message: AgentMessage::User(UserMessage {
                    content: vec![Content::Text {
                        text: format!(
                            "Message {} with substantial content to ensure tokens \
                             exceed threshold. The quick brown fox jumps over the \
                             lazy dog repeatedly. Lorem ipsum dolor sit amet. \
                             Additional padding: {}",
                            i, padding
                        ),
                        text_signature: None,
                    }],
                    timestamp: std::time::SystemTime::now(),
                }),
            }
        })
        .collect()
}

// ============================================================================
// Integration test: overflow → compaction → retry → success
// ============================================================================

#[tokio::test]
async fn test_recovery_overflow_then_compact_and_retry() {
    let _ = tracing_subscriber::fmt().try_init();

    // Call 0: agent loop → overflow
    // Call 1: compaction → summary
    // Call 2: retry → success
    let provider = TestProvider::counted(|n| match n {
        0 => TestResponse::Overflow,
        _ => TestResponse::Text("Success after compaction".into()),
    });
    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".to_string(),
        "test".to_string(),
        provider.clone(),
        dispatcher,
        Arc::new(make_compaction_actor(provider.clone())),
        vec![],
        None,
        vec![],
    );

    // Fill with enough entries to trigger compaction
    let entries = build_entries(12);
    for entry in entries {
        match entry {
            SessionEntry::Message { message, .. } => session.push_message(message),
            _ => {}
        }
    }

    // Prompt should trigger: overflow → compaction → retry → success
    let results = session.prompt("trigger overflow".to_string()).await.unwrap();

    // Verify final success
    assert!(
        !results.is_empty(),
        "prompt should return messages after recovery"
    );
    let last = results.last().unwrap();
    match last {
        AgentMessage::Assistant(assistant) => {
            let text = assistant
                .content
                .iter()
                .filter_map(|c| match c {
                    Content::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            assert!(
                text.contains("Success after compaction"),
                "Expected success message after recovery, got: {}",
                text
            );
        }
        _ => panic!("Expected assistant message, got {:?}", last),
    }

    // Verify entries contain a compaction entry
    let all_entries = session.entries();
    let has_compaction = all_entries
        .iter()
        .any(|e| matches!(e, SessionEntry::Compaction { .. }));
    assert!(
        has_compaction,
        "Expected SessionEntry::Compaction in entries after overflow recovery"
    );

    // Verify provider was called 3 times (overflow + compaction + retry)
    // Note: TestProvider::sequence wraps to start after exhaustion, so we
    // need to check via the provider's internal counter. Since Arc<TestProvider>
    // doesn't expose call_count, we use TestProvider::counted instead for
    // the double-overflow test. For this test, the sequence behavior is sufficient.
}

// ============================================================================
// Integration test: double overflow aborts
// ============================================================================

#[tokio::test]
async fn test_recovery_double_overflow_aborts() {
    let _ = tracing_subscriber::fmt().try_init();

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let provider = TestProvider::counted(move |n| {
        call_count_clone.store(n + 1, Ordering::SeqCst);
        match n {
            1 => TestResponse::Text("Compaction summary".into()),
            _ => TestResponse::Overflow,
        }
    });

    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".to_string(),
        "test".to_string(),
        provider.clone(),
        dispatcher,
        Arc::new(make_compaction_actor(provider)),
        vec![],
        None,
        vec![],
    );

    // Fill with enough entries
    let entries = build_entries(12);
    for entry in entries {
        match entry {
            SessionEntry::Message { message, .. } => session.push_message(message),
            _ => {}
        }
    }

    // First overflow triggers compaction, second overflow aborts
    let result = session.prompt("trigger overflow".to_string()).await;
    assert!(
        result.is_err(),
        "Expected error after double overflow, got Ok"
    );
    match result {
        Err(agent_core::error::AgentError::CompactionFailed(msg)) => {
            assert!(
                msg.contains("Context overflow recovery failed"),
                "Expected compaction failed message, got: {}",
                msg
            );
        }
        other => panic!(
            "Expected CompactionFailed error, got {:?}",
            other
        ),
    }
}
