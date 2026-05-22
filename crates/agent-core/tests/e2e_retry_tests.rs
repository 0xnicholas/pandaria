//! End-to-end retry integration tests: verify that HTTP 429/503 responses
//! trigger exponential backoff inside ai-provider's RequestBuilder, and that
//! AgentLoop's RecoveryStateMachine does NOT escalate to compaction when retry
//! eventually succeeds.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use agent_core::test_utils::AllowAllDispatcher;
use agent_core::{
    Compactor, CompactionConfig, DefaultFileOperationExtractor, SessionActor, SessionConfig,
};
use ai_provider::{LlmProvider, StreamOptions, providers::openai::OpenAiProvider};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_compaction_actor(provider: Arc<dyn LlmProvider>) -> Arc<Compactor> {
    Arc::new(Compactor::new(
        CompactionConfig::default(),
        provider,
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    ))
}

#[tokio::test]
async fn test_retry_429_eventually_succeeds() {
    let _ = tracing_subscriber::fmt().try_init();

    let server = MockServer::start().await;

    let success_body = r#"data: {"id":"chatcmpl-ok","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-ok","object":"chat.completion.chunk","choices":[{"delta":{"content":"Recovered"},"index":0}]}

data: {"id":"chatcmpl-ok","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |_req: &wiremock::Request| {
            let count = call_count_clone.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                // First two calls return 429
                ResponseTemplate::new(429)
                    .set_body_string(r#"{"error":{"message":"rate limit exceeded"}}"#)
            } else {
                ResponseTemplate::new(200).set_body_string(success_body)
            }
        })
        .mount(&server)
        .await;

    let provider: Arc<dyn LlmProvider> = Arc::new(OpenAiProvider::with_base_url(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
    ));

    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "gpt-5.2".to_string(),
        provider: provider.clone(),
        hook_dispatcher: Arc::new(AllowAllDispatcher),
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    // Cap retry delay so the test finishes quickly.
    // RequestBuilder uses base 1s * 2^attempt, capped at max_retry_delay_ms.
    let mut opts = StreamOptions::default();
    opts.max_retry_delay_ms = 100; // clamp each delay to 100ms
    session.set_stream_options(opts);

    let start = tokio::time::Instant::now();
    let result: String = session.complete("trigger retry".to_string()).await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(result, "Recovered");
    // We expect 3 calls: 2 failures + 1 success
    assert_eq!(call_count.load(Ordering::SeqCst), 3);
    // Total retry delay should be ~200ms (2 retries * 100ms cap)
    assert!(
        elapsed >= Duration::from_millis(150),
        "expected at least ~200ms of retry delay, got {:?}",
        elapsed
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "expected test to finish quickly, got {:?}",
        elapsed
    );

    // Verify no compaction was triggered — entries should only contain
    // the user message and the successful assistant response.
    let entries = session.entries();
    assert_eq!(entries.len(), 2);
    assert!(
        entries.iter().all(|e| matches!(
            e,
            agent_core::persistence::entry::SessionEntry::Message { .. }
        )),
        "no compaction entry should appear when retry succeeds"
    );
}

#[tokio::test]
async fn test_non_retryable_error_no_retry() {
    let _ = tracing_subscriber::fmt().try_init();

    let server = MockServer::start().await;

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |_req: &wiremock::Request| {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
            // 401 is non-retryable — RequestBuilder should fail immediately
            ResponseTemplate::new(401).set_body_string(r#"{"error":{"message":"invalid key"}}"#)
        })
        .mount(&server)
        .await;

    let provider: Arc<dyn LlmProvider> = Arc::new(OpenAiProvider::with_base_url(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
    ));

    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "gpt-5.2".to_string(),
        provider: provider.clone(),
        hook_dispatcher: Arc::new(AllowAllDispatcher),
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    let mut opts = StreamOptions::default();
    opts.max_retries = 3; // plenty of retries available, but 401 is non-retryable
    session.set_stream_options(opts);

    let result = session.complete("trigger auth error".to_string()).await;

    assert!(result.is_err(), "expected error on auth failure");
    // Only 1 call — no retries for 401
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}
