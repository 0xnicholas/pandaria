use llm_client::providers::google::GoogleProvider;
use llm_client::{LlmContext, LlmProvider, StreamOptions, StopReason, AssistantMessageEvent};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_mock_basic_text_stream() {
    let server = MockServer::start().await;

    let sse_body = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}

data: {"candidates":[{"content":{"parts":[{"text":" world"}]}}]}

data: {"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}]}

"#;

    Mock::given(method("POST"))
        .and(path("/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider = GoogleProvider::with_base_url(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
    );

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut stream = provider
        .stream("gemini-2.5-pro", ctx, StreamOptions::default(), CancellationToken::new())
        .await
        .expect("stream should start");

    let event = stream.next().await.expect("should have Start");
    assert!(matches!(event, AssistantMessageEvent::Start { .. }));

    let event = stream.next().await.expect("should have TextDelta 'Hello'");
    assert!(matches!(&event, AssistantMessageEvent::TextDelta { delta, content_index: 0, .. } if delta == "Hello"));

    let event = stream.next().await.expect("should have TextDelta ' world'");
    assert!(matches!(&event, AssistantMessageEvent::TextDelta { delta, content_index: 0, .. } if delta == " world"));

    let event = stream.next().await.expect("should have Done");
    match event {
        AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::Stop),
        _ => panic!("expected Done"),
    }

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_mock_tool_call() {
    let server = MockServer::start().await;

    let sse_body = r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":{"path":"/x"}}}]}}]}

data: {"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}]}

"#;

    Mock::given(method("POST"))
        .and(path("/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider = GoogleProvider::with_base_url(
        Some(SecretString::new("sk-test".into())),
        &server.uri(),
    );

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut stream = provider
        .stream("gemini-2.5-pro", ctx, StreamOptions::default(), CancellationToken::new())
        .await
        .expect("stream should start");

    let event = stream.next().await.expect("should have Start");
    assert!(matches!(event, AssistantMessageEvent::Start { .. }));

    let event = stream.next().await.expect("should have ToolCallEnd");
    match event {
        AssistantMessageEvent::ToolCallEnd { content_index: 0, tool_call, .. } => {
            assert_eq!(tool_call.name, "read");
            assert_eq!(tool_call.id, "call_0");
            assert_eq!(tool_call.arguments["path"], "/x");
        }
        _ => panic!("expected ToolCallEnd"),
    }

    let event = stream.next().await.expect("should have Done");
    match event {
        AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::Stop),
        _ => panic!("expected Done"),
    }

    assert!(stream.next().await.is_none());
}
