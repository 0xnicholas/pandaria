//! OpenRouter provider integration tests via wiremock.

use ai_provider::providers::openai_compatible::OpenAiCompatibleProvider;
use ai_provider::{
    AssistantMessageEvent, LlmContext, LlmProvider, StopReason, StreamOptions,
};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_mock_basic_text_stream() {
    let server = MockServer::start().await;

    let sse_body = r#"data: {"id":"gen-1","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"gen-1","object":"chat.completion.chunk","choices":[{"delta":{"content":"Hello via OpenRouter"},"index":0}]}

data: {"id":"gen-1","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
        "openrouter",
        "OPENROUTER_API_KEY",
    );

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut stream = provider
        .stream(
            "anthropic/claude-sonnet-4",
            ctx,
            StreamOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("stream should start");

    let event = stream.next().await.expect("should have Start");
    assert!(matches!(event, AssistantMessageEvent::Start { .. }));

    let event = stream.next().await.expect("should have TextStart");
    assert!(matches!(event, AssistantMessageEvent::TextStart { content_index: 0, .. }));

    let event = stream.next().await.expect("should have TextDelta");
    assert!(
        matches!(&event, AssistantMessageEvent::TextDelta { delta, content_index: 0, .. } if delta == "Hello via OpenRouter")
    );

    let event = stream.next().await.expect("should have TextEnd");
    assert!(matches!(event, AssistantMessageEvent::TextEnd { content_index: 0, .. }));

    let event = stream.next().await.expect("should have Done");
    match event {
        AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::Stop),
        _ => panic!("expected Done"),
    }

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_request_body_has_correct_model_and_provider_name() {
    let server = MockServer::start().await;

    let body_valid = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let body_valid_clone = body_valid.clone();

    let sse_body = r#"data: {"id":"gen-2","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"gen-2","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let model = body["model"].as_str().unwrap_or("");
            let has_auth = req
                .headers
                .get("Authorization")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.contains("sk-test"))
                .unwrap_or(false);
            if model == "anthropic/claude-sonnet-4" && has_auth {
                body_valid_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            ResponseTemplate::new(200).set_body_string(sse_body)
        })
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
        "openrouter",
        "OPENROUTER_API_KEY",
    );

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut stream = provider
        .stream(
            "anthropic/claude-sonnet-4",
            ctx,
            StreamOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("stream should start");

    while stream.next().await.is_some() {}

    assert!(
        body_valid.load(std::sync::atomic::Ordering::SeqCst),
        "OpenRouter request body should preserve the full model spec and include auth header"
    );
}

#[tokio::test]
async fn test_provider_name_override_is_reflected_in_events() {
    let server = MockServer::start().await;

    let sse_body = r#"data: {"id":"gen-3","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"gen-3","object":"chat.completion.chunk","choices":[{"delta":{"content":"ok"},"index":0}]}

data: {"id":"gen-3","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
        "openrouter",
        "OPENROUTER_API_KEY",
    );

    assert_eq!(provider.provider_name(), "openrouter");

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut stream = provider
        .stream(
            "anthropic/claude-sonnet-4",
            ctx,
            StreamOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("stream should start");

    let event = stream.next().await.expect("should have Start");
    match event {
        AssistantMessageEvent::Start { partial } => {
            assert_eq!(partial.provider, "openrouter");
            assert_eq!(partial.model, "anthropic/claude-sonnet-4");
        }
        _ => panic!("expected Start"),
    }
}
