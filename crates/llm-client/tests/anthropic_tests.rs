use llm_client::providers::anthropic::AnthropicProvider;
use llm_client::{AssistantMessageEvent, LlmContext, LlmProvider, StopReason, StreamOptions};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_mock_basic_text_stream() {
    let server = MockServer::start().await;

    let sse_body = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_001","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}

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

    let provider =
        AnthropicProvider::with_base_url(Some(SecretString::new("sk-test".into())), &server.uri());

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
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

    let event = stream.next().await.expect("should have TextDelta ' world'");
    assert!(
        matches!(&event, AssistantMessageEvent::TextDelta { delta, content_index: 0, .. } if delta == " world")
    );

    let event = stream.next().await.expect("should have TextEnd");
    assert!(matches!(
        event,
        AssistantMessageEvent::TextEnd {
            content_index: 0,
            ..
        }
    ));

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

    let sse_body = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_002","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","usage":{"input_tokens":5,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_001","name":"read"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"/x\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":15}}

event: message_stop
data: {"type":"message_stop"}

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url(Some(SecretString::new("sk-test".into())), &server.uri());

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
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
        .expect("stream should start");

    let event = stream.next().await.expect("should have Start");
    assert!(matches!(event, AssistantMessageEvent::Start { .. }));

    let event = stream.next().await.expect("should have ToolCallStart");
    assert!(matches!(
        event,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            ..
        }
    ));

    let event = stream
        .next()
        .await
        .expect("should have ToolCallDelta part 1");
    assert!(matches!(
        &event,
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            ..
        }
    ));

    let event = stream
        .next()
        .await
        .expect("should have ToolCallDelta part 2");
    assert!(matches!(
        &event,
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            ..
        }
    ));

    let event = stream.next().await.expect("should have ToolCallEnd");
    match event {
        AssistantMessageEvent::ToolCallEnd {
            content_index: 0,
            tool_call,
            ..
        } => {
            assert_eq!(tool_call.name, "read");
            assert_eq!(tool_call.id, "toolu_001");
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
async fn test_mock_thinking_streaming() {
    let server = MockServer::start().await;

    let sse_body = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_003","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","usage":{"input_tokens":8,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"I need to analyze"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig123"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}

event: message_stop
data: {"type":"message_stop"}

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url(Some(SecretString::new("sk-test".into())), &server.uri());

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
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
        .expect("stream should start");

    let event = stream.next().await.expect("should have Start");
    assert!(matches!(event, AssistantMessageEvent::Start { .. }));

    let event = stream.next().await.expect("should have ThinkingStart");
    assert!(matches!(
        event,
        AssistantMessageEvent::ThinkingStart {
            content_index: 0,
            ..
        }
    ));

    let event = stream.next().await.expect("should have ThinkingDelta");
    assert!(
        matches!(&event, AssistantMessageEvent::ThinkingDelta { delta, content_index: 0, .. } if delta == "I need to analyze")
    );

    let event = stream.next().await.expect("should have ThinkingEnd");
    match event {
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            thinking,
            ..
        } => {
            assert_eq!(thinking, "I need to analyze");
        }
        _ => panic!("expected ThinkingEnd"),
    }

    let event = stream.next().await.expect("should have Done");
    match event {
        AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::Stop),
        _ => panic!("expected Done"),
    }

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_mock_error_response() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(401).set_body_string(
            r#"{"error":{"type":"authentication_error","message":"Invalid API key"}}"#,
        ))
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url(Some(SecretString::new("sk-test".into())), &server.uri());

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
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
        .expect("stream should start");

    let event = stream.next().await.expect("should have Error");
    match event {
        AssistantMessageEvent::Error { error } => {
            assert!(error.error_message.is_some());
            let msg = error.error_message.as_ref().unwrap();
            assert!(
                msg.contains("401") || msg.contains("Invalid"),
                "error message should contain 401 or Invalid: {}",
                msg
            );
        }
        _ => panic!("expected Error event, got {:?}", event),
    }

    assert!(stream.next().await.is_none());
}
