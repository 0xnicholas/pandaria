use ai_provider::providers::deepseek::DeepSeekProvider;
use ai_provider::{AssistantMessageEvent, LlmContext, LlmProvider, StopReason, StreamOptions};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_mock_basic_text_stream() {
    let server = MockServer::start().await;

    let sse_body = r#"data: {"id":"chatcmpl-ds1","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-ds1","object":"chat.completion.chunk","choices":[{"delta":{"content":"Hello"},"index":0}]}

data: {"id":"chatcmpl-ds1","object":"chat.completion.chunk","choices":[{"delta":{"content":" from"},"index":0}]}

data: {"id":"chatcmpl-ds1","object":"chat.completion.chunk","choices":[{"delta":{"content":" DeepSeek"},"index":0}]}

data: {"id":"chatcmpl-ds1","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    // Pass api_key explicitly for this test
    let provider =
        DeepSeekProvider::with_base_url(Some(SecretString::new("sk-test".into())), &server.uri());

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut stream = provider
        .stream(
            "deepseek-v4-pro",
            ctx,
            StreamOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("stream should start");

    let event = stream.next().await.expect("should have Start");
    assert!(matches!(event, AssistantMessageEvent::Start { .. }));

    let event = stream.next().await.expect("should have TextStart");
    assert!(matches!(
        event,
        AssistantMessageEvent::TextStart {
            content_index: 0,
            ..
        }
    ));

    let event = stream.next().await.expect("should have TextDelta 'Hello'");
    assert!(
        matches!(&event, AssistantMessageEvent::TextDelta { delta, content_index: 0, .. } if delta == "Hello")
    );

    let event = stream.next().await.expect("should have TextDelta ' from'");
    assert!(
        matches!(&event, AssistantMessageEvent::TextDelta { delta, content_index: 0, .. } if delta == " from")
    );

    let event = stream
        .next()
        .await
        .expect("should have TextDelta ' DeepSeek'");
    assert!(
        matches!(&event, AssistantMessageEvent::TextDelta { delta, content_index: 0, .. } if delta == " DeepSeek")
    );

    let event = stream.next().await.expect("should have TextEnd");
    assert!(
        matches!(&event, AssistantMessageEvent::TextEnd { text, content_index: 0, .. } if text == "Hello from DeepSeek")
    );

    let event = stream.next().await.expect("should have Done");
    match event {
        AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::Stop),
        _ => panic!("expected Done"),
    }

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_mock_reasoning_stream() {
    let server = MockServer::start().await;

    // DeepSeek returns reasoning_content in the delta
    let sse_body = r#"data: {"id":"chatcmpl-ds2","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-ds2","object":"chat.completion.chunk","choices":[{"delta":{"reasoning_content":"Let me think"},"index":0}]}

data: {"id":"chatcmpl-ds2","object":"chat.completion.chunk","choices":[{"delta":{"reasoning_content":" about this"},"index":0}]}

data: {"id":"chatcmpl-ds2","object":"chat.completion.chunk","choices":[{"delta":{"content":"The answer is 42"},"index":0}]}

data: {"id":"chatcmpl-ds2","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider =
        DeepSeekProvider::with_base_url(Some(SecretString::new("sk-test".into())), &server.uri());

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut stream = provider
        .stream(
            "deepseek-reasoner",
            ctx,
            StreamOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("stream should start");

    let event = stream.next().await.expect("should have Start");
    assert!(matches!(event, AssistantMessageEvent::Start { .. }));

    let event = stream.next().await.expect("should have ThinkingDelta 1");
    assert!(
        matches!(&event, AssistantMessageEvent::ThinkingDelta { delta, content_index: 0, .. } if delta == "Let me think")
    );

    let event = stream.next().await.expect("should have ThinkingDelta 2");
    assert!(
        matches!(&event, AssistantMessageEvent::ThinkingDelta { delta, content_index: 0, .. } if delta == " about this")
    );

    let event = stream.next().await.expect("should have TextStart");
    assert!(matches!(
        event,
        AssistantMessageEvent::TextStart {
            content_index: 0,
            ..
        }
    ));

    let event = stream.next().await.expect("should have TextDelta");
    assert!(
        matches!(&event, AssistantMessageEvent::TextDelta { delta, content_index: 0, .. } if delta == "The answer is 42")
    );

    let event = stream.next().await.expect("should have TextEnd");
    assert!(
        matches!(&event, AssistantMessageEvent::TextEnd { text, content_index: 0, .. } if text == "The answer is 42")
    );

    let event = stream.next().await.expect("should have Done");
    match event {
        AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::Stop),
        _ => panic!("expected Done"),
    }

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_env_api_key_is_used_in_request() {
    let server = MockServer::start().await;

    let sse_body = r#"data: {"id":"chatcmpl-ds3","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-ds3","object":"chat.completion.chunk","choices":[{"delta":{"content":"ok"},"index":0}]}

data: {"id":"chatcmpl-ds3","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    // Expect the Authorization header with the key from DEEPSEEK_API_KEY env var
    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("Authorization", "Bearer sk-from-env"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    // Set the environment variable for this test
    // SAFETY: tests run sequentially within the same process; this is the
    // only test that touches DEEPSEEK_API_KEY.
    unsafe {
        std::env::set_var("DEEPSEEK_API_KEY", "sk-from-env");
    }

    // Do NOT pass api_key — provider should fall back to env var
    let provider = DeepSeekProvider::with_base_url(None, &server.uri());

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut stream = provider
        .stream(
            "deepseek-chat",
            ctx,
            StreamOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("stream should start");

    let event = stream.next().await.expect("should have Start");
    assert!(matches!(event, AssistantMessageEvent::Start { .. }));

    let event = stream.next().await.expect("should have TextStart");
    assert!(matches!(
        event,
        AssistantMessageEvent::TextStart {
            content_index: 0,
            ..
        }
    ));

    let event = stream.next().await.expect("should have TextDelta 'ok'");
    assert!(
        matches!(&event, AssistantMessageEvent::TextDelta { delta, content_index: 0, .. } if delta == "ok")
    );

    let event = stream.next().await.expect("should have TextEnd");
    assert!(
        matches!(&event, AssistantMessageEvent::TextEnd { text, content_index: 0, .. } if text == "ok")
    );

    let event = stream.next().await.expect("should have Done");
    match event {
        AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::Stop),
        _ => panic!("expected Done"),
    }

    assert!(stream.next().await.is_none());

    // Clean up environment variable
    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
    }
}

/// Real API smoke test — connects to DeepSeek production endpoint.
///
/// Requires `DEEPSEEK_API_KEY` to be set (e.g. via `.env` file).
/// Run with:
///   cargo test -p ai-provider --test deepseek_tests test_real_api -- --ignored
#[tokio::test]
#[ignore]
async fn test_real_api_basic_text() {
    // Load .env file so DEEPSEEK_API_KEY can be picked up when running locally.
    let _ = dotenvy::dotenv();

    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .expect("DEEPSEEK_API_KEY must be set for this test (e.g. in .env file)");

    let provider = DeepSeekProvider::new(Some(SecretString::new(api_key.into())));

    let ctx = LlmContext {
        system_prompt: Some("You are a helpful assistant. Reply with a single word.".to_string()),
        messages: vec![ai_provider::Message::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: "Say hello".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })],
        tools: None,
    };

    let mut stream = provider
        .stream(
            "deepseek-chat",
            ctx,
            StreamOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("stream should start");

    let mut text_parts = Vec::new();

    while let Some(event) = stream.next().await {
        match event {
            AssistantMessageEvent::TextDelta { delta, .. } => {
                text_parts.push(delta);
            }
            AssistantMessageEvent::Done { .. } => break,
            AssistantMessageEvent::Error { error } => {
                panic!("stream error: {:?}", error);
            }
            _ => {}
        }
    }

    let full_text = text_parts.join("").to_lowercase();
    assert!(
        full_text.contains("hello"),
        "expected response to contain 'hello', got: '{}'",
        full_text
    );
}
