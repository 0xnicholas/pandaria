use ai_provider::types::{Content, Message, ToolDef, UserMessage};
use ai_provider::{
    LlmContext, LlmProvider, StreamOptions, providers::anthropic::AnthropicProvider,
    providers::openai::OpenAiProvider,
};
use secrecy::SecretString;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Verify Anthropic request body structure.
#[tokio::test]
async fn test_anthropic_request_body_structure() {
    let server = MockServer::start().await;
    let body_was_valid = Arc::new(AtomicBool::new(false));
    let b = body_was_valid.clone();

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            // Verify required Anthropic fields
            assert_eq!(body["model"], "claude-sonnet-4-20250514");
            assert!(body["max_tokens"].as_u64().unwrap() > 0);
            assert!(body["messages"].is_array());
            assert!(body["messages"][0]["role"] == "user");
            assert!(body["messages"][0]["content"].is_array());

            b.store(true, Ordering::SeqCst);
            // Return a minimal valid SSE stream
            ResponseTemplate::new(200)
                .set_body_string("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"test\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n")
        })
        .mount(&server)
        .await;

    let api_key = SecretString::new("test-key".into());
    let provider =
        AnthropicProvider::with_base_url(Some(api_key), &format!("{}/v1/messages", server.uri()));

    let ctx = LlmContext {
        system_prompt: Some("You are a helpful assistant.".into()),
        messages: vec![Message::User(UserMessage {
            content: vec![Content::Text {
                text: "hello".into(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })],
        tools: Some(vec![ToolDef {
            name: "test_tool".into(),
            description: "A test tool".into(),
            parameters: json!({"type": "object", "properties": {"x": {"type": "string"}}}),
        }]),
    };

    let mut stream = provider
        .stream(
            "claude-sonnet-4-20250514",
            ctx,
            StreamOptions::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    // Drain the stream
    while stream.next().await.is_some() {}

    assert!(body_was_valid.load(Ordering::SeqCst));
}

/// Verify OpenAI request body structure.
#[tokio::test]
async fn test_openai_request_body_structure() {
    let server = MockServer::start().await;
    let body_was_valid = Arc::new(AtomicBool::new(false));
    let b = body_was_valid.clone();

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(body["model"], "gpt-5.2");
            assert!(body["messages"].is_array());
            assert!(body["messages"][0]["role"] == "user");
            assert!(body["tools"].is_array());
            b.store(true, Ordering::SeqCst);
            ResponseTemplate::new(200)
                .set_body_string("data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n")
        })
        .mount(&server)
        .await;

    let provider = OpenAiProvider::with_base_url(
        Some(SecretString::new("test-key".into())),
        &format!("{}/v1/chat/completions", server.uri()),
    );

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![Message::User(UserMessage {
            content: vec![Content::Text {
                text: "hello".into(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })],
        tools: Some(vec![ToolDef {
            name: "test_tool".into(),
            description: "test".into(),
            parameters: json!({"type": "object"}),
        }]),
    };

    let mut stream = provider
        .stream(
            "gpt-5.2",
            ctx,
            StreamOptions::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    while stream.next().await.is_some() {}
    assert!(body_was_valid.load(Ordering::SeqCst));
}

/// Verify that `with_client()` allows injecting a shared reqwest::Client
/// for connection pooling.
#[tokio::test]
async fn test_provider_with_shared_client() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"test\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n"),
        )
        .mount(&server)
        .await;

    // Create a shared client with custom timeout
    let shared_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    // Inject the shared client into AnthropicProvider
    let provider = AnthropicProvider::with_client(
        shared_client,
        Some(SecretString::new("test-key".into())),
        &format!("{}/v1/messages", server.uri()),
    );

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![Message::User(UserMessage {
            content: vec![Content::Text {
                text: "hello".into(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })],
        tools: None,
    };

    let mut stream = provider
        .stream(
            "claude-sonnet-4-20250514",
            ctx,
            StreamOptions::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    // Drain the stream
    let mut event_count = 0;
    while stream.next().await.is_some() {
        event_count += 1;
    }
    assert!(event_count >= 2); // At least Start + Done
}
