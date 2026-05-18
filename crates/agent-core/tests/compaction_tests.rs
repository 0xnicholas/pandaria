use agent_core::harness::compaction::{CompactionActor, CompactionConfig, should_compact};
use agent_core::error::CompactionError;
use agent_core::file_ops::DefaultFileOperationExtractor;
use agent_core::hook::dispatcher::HookDispatcher;
use agent_core::persistence::entry::{CompactionDetails, SessionEntry};
use agent_core::types::AgentMessage;
use agent_core::{SessionActor, SessionConfig};
use ai_provider::{
    Api, AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, Content, LlmContext,
    LlmError, LlmProvider, StopReason, StreamOptions, ToolCall, Usage, UserMessage,
};
use std::sync::Arc;
use std::time::SystemTime;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// ============================================================================
// Should compact tests (sync)
// ============================================================================

#[test]
fn test_should_compact_enabled() {
    let config = CompactionConfig::new(true, 1000, 500);
    // context_window = 10000, context_tokens = 9500
    // 9500 > 10000 - 1000 = 9000, should trigger
    assert!(should_compact(9500, 10000, &config));
    // 8000 > 9000? No
    assert!(!should_compact(8000, 10000, &config));
}

#[test]
fn test_should_compact_disabled() {
    let config = CompactionConfig::new(false, 1000, 500);
    assert!(!should_compact(9500, 10000, &config));
}

// ============================================================================
// CompactionActor preparation tests (sync)
// ============================================================================

fn make_test_provider() -> Arc<dyn ai_provider::LlmProvider> {
    Arc::new(TestProvider)
}

fn test_provider_config() -> &'static ai_provider::providers::shared::ProviderConfig {
    use std::sync::OnceLock;
    static CONFIG: OnceLock<ai_provider::providers::shared::ProviderConfig> = OnceLock::new();
    CONFIG.get_or_init(|| {
        ai_provider::providers::shared::ProviderConfig::new(
            None, "http://test", "test", "TEST_API_KEY",
        )
    })
}

struct TestProvider;
#[async_trait::async_trait]
impl ai_provider::LlmProvider for TestProvider {
    fn provider_name(&self) -> &str {
        "test"
    }
    fn models(&self) -> Vec<String> {
        vec!["test".to_string()]
    }
    fn config(&self) -> &ai_provider::providers::shared::ProviderConfig {
        test_provider_config()
    }
    async fn stream(
        &self,
        _model: &str,
        _context: ai_provider::LlmContext,
        _options: ai_provider::StreamOptions,
        _signal: tokio_util::sync::CancellationToken,
    ) -> Result<ai_provider::AssistantMessageEventStream, ai_provider::LlmError> {
        unreachable!()
    }
}

#[test]
fn test_compaction_actor_prepare_empty() {
    let actor = CompactionActor::new(
        CompactionConfig::new(true, 1000, 500),
        make_test_provider(),
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    );

    let entries: Vec<SessionEntry> = vec![];
    let result = actor.prepare(&entries);
    assert!(result.is_err());
}

#[test]
fn test_compaction_actor_prepare_single_message() {
    let actor = CompactionActor::new(
        CompactionConfig::new(true, 1000, 500),
        make_test_provider(),
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    );

    let entries = vec![SessionEntry::Message {
        id: Uuid::new_v4(),
        message: AgentMessage::User(UserMessage {
            content: vec![Content::Text {
                text: "Hello".to_string(),
                text_signature: None,
            }],
            timestamp: SystemTime::now(),
        }),
    }];

    let prep = actor.prepare(&entries).unwrap();
    // With a single short message and large keep_recent, nothing to summarize
    assert!(prep.messages_to_summarize.is_empty());
}

// ============================================================================
// Mock LLM providers for async compact() tests
// ============================================================================

fn default_api() -> Api {
    Api {
        provider: "mock".into(),
        model: "mock".into(),
    }
}

fn default_usage() -> Usage {
    Usage {
        input_tokens: 0,
        output_tokens: 1,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
        total_tokens: 1,
    }
}

fn make_assistant_message(content: Vec<Content>) -> AssistantMessage {
    AssistantMessage {
        content,
        provider: "mock".into(),
        model: "mock".into(),
        api: default_api(),
        usage: default_usage(),
        stop_reason: StopReason::Stop,
        response_id: None,
        error_message: None,
        timestamp: SystemTime::now(),
    }
}

fn mock_provider_with_summary(summary: &str) -> Arc<dyn LlmProvider> {
    struct SummaryProvider {
        summary: String,
    }
    #[async_trait::async_trait]
    impl LlmProvider for SummaryProvider {
        fn provider_name(&self) -> &str {
            "mock-summary"
        }
        fn models(&self) -> Vec<String> {
            vec!["mock".into()]
        }
        fn config(&self) -> &ai_provider::providers::shared::ProviderConfig {
            test_provider_config()
        }
        async fn stream(
            &self,
            _model: &str,
            _ctx: LlmContext,
            _opts: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<AssistantMessageEventStream, LlmError> {
            let partial = make_assistant_message(vec![Content::Text {
                text: self.summary.clone(),
                text_signature: None,
            }]);
            let events = vec![
                AssistantMessageEvent::Start {
                    partial: partial.clone(),
                },
                AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: self.summary.clone(),
                    partial: partial.clone(),
                },
                AssistantMessageEvent::Done {
                    reason: StopReason::Stop,
                    message: partial,
                },
            ];
            Ok(AssistantMessageEventStream::from_events(events))
        }
    }
    Arc::new(SummaryProvider {
        summary: summary.to_string(),
    })
}

fn mock_provider_with_error(error_msg: &str) -> Arc<dyn LlmProvider> {
    struct ErrorProvider {
        error: String,
    }
    #[async_trait::async_trait]
    impl LlmProvider for ErrorProvider {
        fn provider_name(&self) -> &str {
            "mock-error"
        }
        fn models(&self) -> Vec<String> {
            vec!["mock".into()]
        }
        fn config(&self) -> &ai_provider::providers::shared::ProviderConfig {
            test_provider_config()
        }
        async fn stream(
            &self,
            _model: &str,
            _ctx: LlmContext,
            _opts: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<AssistantMessageEventStream, LlmError> {
            let error_message = AssistantMessage {
                content: vec![],
                provider: "mock".into(),
                model: "mock".into(),
                api: default_api(),
                usage: default_usage(),
                stop_reason: StopReason::Error,
                response_id: None,
                error_message: Some(self.error.clone()),
                timestamp: SystemTime::now(),
            };
            let events = vec![AssistantMessageEvent::Error {
                error: error_message,
            }];
            Ok(AssistantMessageEventStream::from_events(events))
        }
    }
    Arc::new(ErrorProvider {
        error: error_msg.to_string(),
    })
}

fn mock_provider_empty_done() -> Arc<dyn LlmProvider> {
    struct EmptyDoneProvider;
    #[async_trait::async_trait]
    impl LlmProvider for EmptyDoneProvider {
        fn provider_name(&self) -> &str {
            "mock-empty"
        }
        fn models(&self) -> Vec<String> {
            vec!["mock".into()]
        }
        fn config(&self) -> &ai_provider::providers::shared::ProviderConfig {
            test_provider_config()
        }
        async fn stream(
            &self,
            _model: &str,
            _ctx: LlmContext,
            _opts: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<AssistantMessageEventStream, LlmError> {
            let partial = make_assistant_message(vec![]);
            let events = vec![
                AssistantMessageEvent::Start {
                    partial: partial.clone(),
                },
                AssistantMessageEvent::Done {
                    reason: StopReason::Stop,
                    message: partial,
                },
            ];
            Ok(AssistantMessageEventStream::from_events(events))
        }
    }
    Arc::new(EmptyDoneProvider)
}

// ============================================================================
// Helper: build entries and compaction actor
// ============================================================================

fn make_compaction_actor(provider: Arc<dyn LlmProvider>) -> CompactionActor {
    CompactionActor::new(
        CompactionConfig::new(true, 1000, 100),
        provider,
        "mock".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    )
}

fn user_entry(text: &str) -> SessionEntry {
    SessionEntry::Message {
        id: Uuid::new_v4(),
        message: AgentMessage::User(UserMessage {
            content: vec![Content::Text {
                text: text.to_string(),
                text_signature: None,
            }],
            timestamp: SystemTime::now(),
        }),
    }
}

fn assistant_with_tool_call(tool_name: &str, path: &str) -> SessionEntry {
    let mut args = serde_json::Map::new();
    args.insert("path".to_string(), serde_json::Value::String(path.to_string()));
    SessionEntry::Message {
        id: Uuid::new_v4(),
        message: AgentMessage::Assistant(AssistantMessage {
            content: vec![Content::ToolCall(ToolCall {
                id: format!("tc_{}", Uuid::new_v4()),
                name: tool_name.to_string(),
                arguments: serde_json::Value::Object(args),
                thought_signature: None,
            })],
            provider: "mock".into(),
            model: "mock".into(),
            api: default_api(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 10,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 20,
            },
            stop_reason: StopReason::ToolUse,
            response_id: None,
            error_message: None,
            timestamp: SystemTime::now(),
        }),
    }
}

/// Build N entries with sufficient text to trigger summarization.
/// Each entry is ~300 chars (~75 tokens). With keep_recent=100,
/// the last ~2 entries are kept and the rest summarized.
fn build_many_entries(count: usize) -> Vec<SessionEntry> {
    (0..count)
        .map(|i| {
            let padding = "x".repeat(80);
            user_entry(&format!(
                "User message {}. We need substantial text here to ensure enough tokens \
                 for compaction. The quick brown fox jumps over the lazy dog. \
                 More padding to reach token threshold. Extra content: lorem ipsum \
                 dolor sit amet consectetur adipiscing elit. Additional padding: {}",
                i, padding
            ))
        })
        .collect()
}

fn make_compaction_entry(summary: &str, first_kept_entry_id: Uuid) -> SessionEntry {
    SessionEntry::Compaction {
        id: Uuid::new_v4(),
        summary: summary.to_string(),
        first_kept_entry_id,
        tokens_before: 500,
        details: Some(CompactionDetails {
            read_files: vec![],
            modified_files: vec![],
        }),
        from_extension: false,
        timestamp: SystemTime::now(),
    }
}

// ============================================================================
// Integration tests: compact()
// ============================================================================

#[tokio::test]
async fn test_compact_basic_success() {
    let expected_summary = "This is a test summary of the conversation.";
    let provider = mock_provider_with_summary(expected_summary);
    let actor = make_compaction_actor(provider);
    let entries = build_many_entries(10);

    let result = actor.compact(&entries, &CancellationToken::new()).await.unwrap();

    assert_eq!(result.summary, expected_summary);
    assert!(!result.first_kept_entry_id.is_nil());
    assert!(result.tokens_before > 0);
    assert!(result.details.is_some());

    // Verify SessionEntry::Compaction can be constructed from result
    let compaction_entry = SessionEntry::Compaction {
        id: Uuid::new_v4(),
        summary: result.summary.clone(),
        first_kept_entry_id: result.first_kept_entry_id,
        tokens_before: result.tokens_before,
        details: result.details.clone(),
        from_extension: false,
        timestamp: SystemTime::now(),
    };
    assert_eq!(compaction_entry.id().to_string().len(), 36);
    // Verify we can retrieve fields back from the entry
    match compaction_entry {
        SessionEntry::Compaction {
            summary,
            first_kept_entry_id,
            tokens_before,
            ..
        } => {
            assert_eq!(summary, expected_summary);
            assert_eq!(first_kept_entry_id, result.first_kept_entry_id);
            assert_eq!(tokens_before, result.tokens_before);
        }
        _ => panic!("Expected Compaction entry"),
    }
}

#[tokio::test]
async fn test_compact_llm_error() {
    let provider = mock_provider_with_error("LLM API failure");
    let actor = make_compaction_actor(provider);
    let entries = build_many_entries(10);

    let result = actor.compact(&entries, &CancellationToken::new()).await;

    match result {
        Err(CompactionError::LlmError(msg)) => {
            assert!(msg.contains("LLM API failure"), "unexpected error: {msg}");
        }
        other => panic!("Expected CompactionError::LlmError, got {other:?}"),
    }
}

#[tokio::test]
async fn test_compact_empty_summary_returns_error() {
    let provider = mock_provider_empty_done();
    let actor = make_compaction_actor(provider);
    let entries = build_many_entries(10);

    let result = actor.compact(&entries, &CancellationToken::new()).await;

    match result {
        Err(CompactionError::LlmError(msg)) => {
            assert!(
                msg.contains("empty") || msg.contains("Empty"),
                "unexpected error: {msg}"
            );
        }
        other => panic!("Expected CompactionError::LlmError, got {other:?}"),
    }
}

#[tokio::test]
async fn test_compact_with_tool_calls_populates_details() {
    let expected_summary = "Summary with tool calls.";
    let provider = mock_provider_with_summary(expected_summary);
    let actor = make_compaction_actor(provider);

    // Build entries with tool calls in between user messages
    let mut entries = build_many_entries(5);
    // Add assistant entries with tool calls that DefaultFileOperationExtractor recognizes
    entries.push(user_entry("Please read the main file and write to output."));
    entries.push(assistant_with_tool_call("read", "src/main.rs"));
    entries.push(user_entry("Now edit the config."));
    entries.push(assistant_with_tool_call("edit", "config.toml"));
    entries.push(user_entry("Write the output file."));
    entries.push(assistant_with_tool_call("write", "target/output.txt"));
    entries.push(user_entry("Now check the utils."));
    entries.push(assistant_with_tool_call("read", "src/utils.rs"));
    entries.push(user_entry("Done."));
    // Add padding entries to ensure summarization triggers
    entries.extend(build_many_entries(5));

    let result = actor.compact(&entries, &CancellationToken::new()).await.unwrap();

    assert_eq!(result.summary, expected_summary);

    let details = result.details.expect("details should be present");
    // DefaultFileOperationExtractor: read_tool_names = ["read"], write_tool_names = ["write"]
    assert!(
        details.read_files.contains(&"src/main.rs".to_string()),
        "read_files should contain src/main.rs, got {:?}",
        details.read_files
    );
    assert!(
        details.read_files.contains(&"src/utils.rs".to_string()),
        "read_files should contain src/utils.rs, got {:?}",
        details.read_files
    );
    assert!(
        details.modified_files.contains(&"target/output.txt".to_string()),
        "modified_files should contain target/output.txt, got {:?}",
        details.modified_files
    );
    // Edit tools are tracked separately (edited), not in modified_files
    assert!(
        !details.modified_files.contains(&"config.toml".to_string()),
        "edited files should not appear in modified_files"
    );
}

#[tokio::test]
async fn test_compact_with_previous_compaction() {
    let expected_summary = "Updated summary incorporating new messages.";
    let provider = mock_provider_with_summary(expected_summary);
    let actor = make_compaction_actor(provider);

    // Previous compaction with a known first_kept_entry_id
    let kept_id = Uuid::new_v4();
    let mut entries = vec![
        user_entry("Old message that was already compacted."),
        user_entry("Another old message."),
        make_compaction_entry("Previous summary of old conversation.", kept_id),
        SessionEntry::Message {
            id: kept_id,
            message: AgentMessage::User(UserMessage {
                content: vec![Content::Text {
                    text: "First message after previous compaction.".into(),
                    text_signature: None,
                }],
                timestamp: SystemTime::now(),
            }),
        },
    ];
    // Add enough new messages to trigger another compaction
    entries.extend(build_many_entries(10));

    let result = actor.compact(&entries, &CancellationToken::new()).await.unwrap();

    assert_eq!(result.summary, expected_summary);
    assert!(result.tokens_before > 0);
    // The first_kept_entry_id should be from one of the new messages (after the
    // previous compaction boundary), not the old kept_id
    assert_ne!(result.first_kept_entry_id, kept_id);
}

#[tokio::test]
async fn test_compact_empty_entries_returns_error() {
    let provider = mock_provider_with_summary("should not be used");
    let actor = make_compaction_actor(provider);
    let entries: Vec<SessionEntry> = vec![];

    let result = actor.compact(&entries, &CancellationToken::new()).await;

    match result {
        Err(CompactionError::AlreadyCompacted) => {}
        other => panic!("Expected CompactionError::AlreadyCompacted, got {other:?}"),
    }
}

#[tokio::test]
async fn test_compact_session_entry_direct_construction() {
    // Verify all fields map correctly from CompactionResult to SessionEntry::Compaction
    let first_kept = Uuid::new_v4();
    let details = CompactionDetails {
        read_files: vec!["src/main.rs".into()],
        modified_files: vec!["src/output.rs".into()],
    };

    let result = agent_core::harness::compaction::CompactionResult {
        summary: "Test summary".to_string(),
        first_kept_entry_id: first_kept,
        tokens_before: 1200,
        details: Some(details.clone()),
    };

    let entry_id = Uuid::new_v4();
    let entry = SessionEntry::Compaction {
        id: entry_id,
        summary: result.summary.clone(),
        first_kept_entry_id: result.first_kept_entry_id,
        tokens_before: result.tokens_before,
        details: result.details.clone(),
        from_extension: false,
        timestamp: SystemTime::now(),
    };

    assert_eq!(entry.id(), entry_id);
    let SessionEntry::Compaction {
        id,
        summary,
        first_kept_entry_id,
        tokens_before,
        details: d,
        from_extension,
        ..
    } = &entry
    else {
        panic!("Expected Compaction entry");
    };

    assert_eq!(*id, entry_id);
    assert_eq!(summary, "Test summary");
    assert_eq!(*first_kept_entry_id, first_kept);
    assert_eq!(*tokens_before, 1200);
    assert_eq!(d.as_ref().unwrap().read_files, details.read_files);
    assert_eq!(d.as_ref().unwrap().modified_files, details.modified_files);
    assert!(!from_extension);
}

// ============================================================================
// AllowAllDispatcher for SessionActor tests
// ============================================================================

struct AllowAllDispatcher;
#[async_trait::async_trait]
impl HookDispatcher for AllowAllDispatcher {}

// ============================================================================
// Integration test: compact() via SessionActor writes SessionEntry
// ============================================================================

#[tokio::test]
async fn test_compact_via_session_actor_writes_entry() {
    let _ = tracing_subscriber::fmt().try_init();
    let provider = mock_provider_with_summary("Session compaction summary");
    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "test".to_string(),
        provider: provider.clone(),
        hook_dispatcher: dispatcher,
        compaction_actor: Arc::new(make_compaction_actor(provider)),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    // Fill entries with sufficient text to trigger summarization
    let entries = build_many_entries(10);
    for entry in entries {
        match entry {
            SessionEntry::Message { message, .. } => session.push_message(message),
            _ => {}
        }
    }

    let result = session.compact(None).await.unwrap();
    assert_eq!(result.summary, "Session compaction summary");

    let all_entries = session.entries();
    assert!(
        all_entries.len() > 0,
        "session should have entries after compaction"
    );

    let last = all_entries.last().unwrap();
    match last {
        SessionEntry::Compaction {
            summary,
            first_kept_entry_id,
            tokens_before,
            details,
            from_extension,
            ..
        } => {
            assert_eq!(summary, "Session compaction summary");
            assert!(!first_kept_entry_id.is_nil());
            assert!(tokens_before > &0);
            assert!(details.is_some());
            assert!(!from_extension);
        }
        _ => panic!("Expected SessionEntry::Compaction, got {:?}", last),
    }
}

#[tokio::test]
async fn test_compact_truncates_old_entries() {
    let _ = tracing_subscriber::fmt().try_init();
    let provider = mock_provider_with_summary("Truncation test summary");
    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "test".to_string(),
        provider: provider.clone(),
        hook_dispatcher: dispatcher,
        compaction_actor: Arc::new(make_compaction_actor(provider)),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    let entries = build_many_entries(10);
    let _kept_id = entries[8].id(); // Keep reference to an entry near the end
    for entry in entries {
        match entry {
            SessionEntry::Message { message, .. } => session.push_message(message),
            _ => {}
        }
    }

    let before_count = session.entries().len();
    assert_eq!(before_count, 10);

    let result = session.compact(None).await.unwrap();

    let after_entries = session.entries();
    // Old entries before first_kept_entry_id should have been removed.
    // Only kept entries + the new Compaction entry remain.
    assert!(
        after_entries.len() < before_count + 1,
        "entries should have been truncated, before={}, after={}",
        before_count,
        after_entries.len()
    );

    // The first entry in the truncated list should be the first kept entry
    let first_id = after_entries.first().unwrap().id();
    assert_eq!(
        first_id, result.first_kept_entry_id,
        "first remaining entry should match first_kept_entry_id"
    );

    // The last entry should be the Compaction marker
    assert!(
        matches!(after_entries.last().unwrap(), SessionEntry::Compaction { .. }),
        "last entry should be Compaction"
    );
}

#[tokio::test]
async fn test_multiple_compactions_truncate_incrementally() {
    let _ = tracing_subscriber::fmt().try_init();
    let provider = mock_provider_with_summary("Multi compaction summary");
    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "test".to_string(),
        provider: provider.clone(),
        hook_dispatcher: dispatcher,
        compaction_actor: Arc::new(make_compaction_actor(provider)),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    // First batch
    let entries1 = build_many_entries(10);
    for entry in entries1 {
        match entry {
            SessionEntry::Message { message, .. } => session.push_message(message),
            _ => {}
        }
    }

    let _result1 = session.compact(None).await.unwrap();
    let after_first = session.entries().len();

    // Second batch
    let entries2 = build_many_entries(10);
    for entry in entries2 {
        match entry {
            SessionEntry::Message { message, .. } => session.push_message(message),
            _ => {}
        }
    }

    let result2 = session.compact(None).await.unwrap();
    let after_second = session.entries().len();

    // After second compaction, entries from the first kept set of compaction 1
    // that are before compaction 2's kept boundary should be gone.
    // The list should not grow monotonically.
    assert!(
        after_second <= after_first + 3,
        "second compaction should truncate, not accumulate indefinitely: first={}, second={}",
        after_first,
        after_second
    );

    // Verify the first remaining entry matches compaction 2's kept boundary
    let first_id = session.entries().first().unwrap().id();
    assert_eq!(
        first_id, result2.first_kept_entry_id,
        "first remaining entry should match latest first_kept_entry_id"
    );
}
