use llm_client::providers::openai::OpenAiProvider;
use llm_client::{LlmContext, LlmProvider, StreamOptions, StopReason, AssistantMessageEvent};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_mock_basic_text_stream() {
    let server = MockServer::start().await;

    let sse_body = r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"delta":{"content":"Hello"},"index":0}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"delta":{"content":" world"},"index":0}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider = OpenAiProvider::with_base_url(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
    );

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut stream = provider
        .stream("gpt-4.1", ctx, StreamOptions::default(), CancellationToken::new())
        .await
        .expect("stream should start");

    let event = stream.next().await.expect("should have Start");
    assert!(matches!(event, AssistantMessageEvent::Start { .. }));

    let event = stream.next().await.expect("should have TextStart");
    assert!(matches!(event, AssistantMessageEvent::TextStart { content_index: 0, .. }));

    let event = stream.next().await.expect("should have TextDelta 'Hello'");
    assert!(matches!(&event, AssistantMessageEvent::TextDelta { delta, content_index: 0, .. } if delta == "Hello"));

    let event = stream.next().await.expect("should have TextDelta ' world'");
    assert!(matches!(&event, AssistantMessageEvent::TextDelta { delta, content_index: 0, .. } if delta == " world"));

    let event = stream.next().await.expect("should have TextEnd");
    assert!(matches!(&event, AssistantMessageEvent::TextEnd { text, content_index: 0, .. } if text == "Hello world"));

    let event = stream.next().await.expect("should have Done");
    match event {
        AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::Stop),
        _ => panic!("expected Done"),
    }

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_mock_tool_call_streaming() {
    let server = MockServer::start().await;

    let sse_body = r#"data: {"id":"chatcmpl-456","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-456","object":"chat.completion.chunk","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_123","function":{"name":"read"}}]},"index":0}]}

data: {"id":"chatcmpl-456","object":"chat.completion.chunk","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]},"index":0}]}

data: {"id":"chatcmpl-456","object":"chat.completion.chunk","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"/x\"}"}}]},"index":0}]}

data: {"id":"chatcmpl-456","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"tool_calls","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider = OpenAiProvider::with_base_url(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
    );

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut stream = provider
        .stream("gpt-4.1", ctx, StreamOptions::default(), CancellationToken::new())
        .await
        .expect("stream should start");

    let event = stream.next().await.expect("should have Start");
    assert!(matches!(event, AssistantMessageEvent::Start { .. }));

    let event = stream.next().await.expect("should have ToolCallDelta part 1");
    assert!(matches!(&event, AssistantMessageEvent::ToolCallDelta { delta, .. } if delta == "{\"path\":"));

    let event = stream.next().await.expect("should have ToolCallDelta part 2");
    assert!(matches!(&event, AssistantMessageEvent::ToolCallDelta { delta, .. } if delta == "\"/x\"}"));

    let event = stream.next().await.expect("should have ToolCallEnd");
    match event {
        AssistantMessageEvent::ToolCallEnd { tool_call, .. } => {
            assert_eq!(tool_call.name, "read");
            assert_eq!(tool_call.id, "call_123");
            assert_eq!(tool_call.arguments["path"], "/x");
        }
        _ => panic!("expected ToolCallEnd"),
    }

    let event = stream.next().await.expect("should have Done");
    match event {
        AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::ToolUse),
        _ => panic!("expected Done"),
    }

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_mock_reasoning_stream() {
    let server = MockServer::start().await;

    let sse_body = r#"data: {"id":"chatcmpl-789","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-789","object":"chat.completion.chunk","choices":[{"delta":{"reasoning_content":"Let me analyze"},"index":0}]}

data: {"id":"chatcmpl-789","object":"chat.completion.chunk","choices":[{"delta":{"reasoning_content":" this step by step"},"index":0}]}

data: {"id":"chatcmpl-789","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider = OpenAiProvider::with_base_url(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
    );

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut stream = provider
        .stream("gpt-4.1", ctx, StreamOptions::default(), CancellationToken::new())
        .await
        .expect("stream should start");

    let event = stream.next().await.expect("should have Start");
    assert!(matches!(event, AssistantMessageEvent::Start { .. }));

    let event = stream.next().await.expect("should have ThinkingDelta 1");
    assert!(matches!(&event, AssistantMessageEvent::ThinkingDelta { delta, content_index: 0, .. } if delta == "Let me analyze"));

    let event = stream.next().await.expect("should have ThinkingDelta 2");
    assert!(matches!(&event, AssistantMessageEvent::ThinkingDelta { delta, content_index: 0, .. } if delta == " this step by step"));

    let event = stream.next().await.expect("should have Done");
    match event {
        AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::Stop),
        _ => panic!("expected Done"),
    }

    assert!(stream.next().await.is_none());
}
