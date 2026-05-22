//! End-to-end integration tests: agent-core SessionActor + ai-provider concrete
//! implementations (Anthropic, OpenAI) backed by wiremock mock servers.

use std::sync::Arc;

use agent_core::test_utils::AllowAllDispatcher;
use agent_core::types::AgentMessage;
use agent_core::{
    Compactor, CompactionConfig, DefaultFileOperationExtractor, SessionActor, SessionConfig,
};
use ai_provider::{
    Content, LlmProvider,
    providers::{anthropic::AnthropicProvider, openai::OpenAiProvider},
};
use secrecy::SecretString;
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

// ============================================================================
// Anthropic Messages API end-to-end
// ============================================================================

#[tokio::test]
async fn test_anthropic_provider_complete_via_session_actor() {
    let _ = tracing_subscriber::fmt().try_init();

    let server = MockServer::start().await;

    let sse_body = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_001","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello from Anthropic"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}

event: message_stop
data: {"type":"message_stop"}

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider: Arc<dyn LlmProvider> = Arc::new(AnthropicProvider::with_base_url(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
    ));

    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "claude-sonnet-4-20250514".to_string(),
        provider: provider.clone(),
        hook_dispatcher: Arc::new(AllowAllDispatcher),
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    let result: String = session.complete("hi".to_string()).await.unwrap();
    assert_eq!(result, "Hello from Anthropic");

    // Verify entries contain both user and assistant messages
    let entries = session.entries();
    assert_eq!(entries.len(), 2);

    match &entries[0] {
        agent_core::persistence::entry::SessionEntry::Message { message, .. } => {
            assert!(matches!(message, AgentMessage::User(_)));
        }
        _ => panic!("expected user message entry"),
    }

    match &entries[1] {
        agent_core::persistence::entry::SessionEntry::Message { message, .. } => match message {
            AgentMessage::Assistant(a) => {
                let text: String = a
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(text, "Hello from Anthropic");
            }
            _ => panic!("expected assistant message"),
        },
        _ => panic!("expected assistant message entry"),
    }
}

// ============================================================================
// OpenAI Chat Completions API end-to-end
// ============================================================================

#[tokio::test]
async fn test_openai_provider_complete_via_session_actor() {
    let _ = tracing_subscriber::fmt().try_init();

    let server = MockServer::start().await;

    let sse_body = r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"delta":{"content":"Hello from OpenAI"},"index":0}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider: Arc<dyn LlmProvider> = Arc::new(OpenAiProvider::with_base_url(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
    ));

    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s2".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "gpt-5.2".to_string(),
        provider: provider.clone(),
        hook_dispatcher: Arc::new(AllowAllDispatcher),
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    let result: String = session.complete("hi".to_string()).await.unwrap();
    assert_eq!(result, "Hello from OpenAI");

    let entries = session.entries();
    assert_eq!(entries.len(), 2);

    match &entries[1] {
        agent_core::persistence::entry::SessionEntry::Message { message, .. } => match message {
            AgentMessage::Assistant(a) => {
                let text: String = a
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(text, "Hello from OpenAI");
            }
            _ => panic!("expected assistant message"),
        },
        _ => panic!("expected assistant message entry"),
    }
}

// ============================================================================
// Multi-turn conversation via prompt()
// ============================================================================

#[tokio::test]
async fn test_openai_provider_multi_turn_prompt() {
    let _ = tracing_subscriber::fmt().try_init();

    let server = MockServer::start().await;

    // First turn: assistant asks a follow-up
    let sse_body_1 = r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"delta":{"content":"What is your name?"},"index":0}]}

data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    // Second turn: assistant responds
    let sse_body_2 = r#"data: {"id":"chatcmpl-2","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-2","object":"chat.completion.chunk","choices":[{"delta":{"content":"Nice to meet you"},"index":0}]}

data: {"id":"chatcmpl-2","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    let body_counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let body_counter_clone = body_counter.clone();

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let count = body_counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let body = if count == 0 { sse_body_1 } else { sse_body_2 };
            ResponseTemplate::new(200).set_body_string(body)
        })
        .mount(&server)
        .await;

    let provider: Arc<dyn LlmProvider> = Arc::new(OpenAiProvider::with_base_url(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
    ));

    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s3".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "gpt-5.2".to_string(),
        provider: provider.clone(),
        hook_dispatcher: Arc::new(AllowAllDispatcher),
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    let turn1 = session.prompt("Say something".to_string()).await.unwrap();
    assert!(!turn1.is_empty());

    let turn2 = session
        .prompt("My name is Alice".to_string())
        .await
        .unwrap();
    assert!(!turn2.is_empty());

    // Total entries: system prompt (implicit) + 4 messages (user1, assistant1, user2, assistant2)
    // Actually SessionEntry only stores explicit messages; system prompt is in prompt_builder
    let entries = session.entries();
    // Each prompt() adds a user message + the resulting assistant message(s)
    // turn1 adds user("Say something") + assistant("What is your name?")
    // turn2 adds user("My name is Alice") + assistant("Nice to meet you")
    assert_eq!(entries.len(), 4, "expected 4 entries: 2 user + 2 assistant");
}
