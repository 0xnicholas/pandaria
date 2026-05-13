use ai_provider::providers::anthropic::AnthropicProvider;
use ai_provider::{LlmContext, LlmProvider, StreamOptions};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_anthropic_smoke_basic_text() {
    // Start mock server
    let server = MockServer::start().await;

    // SSE body: one text block
    let sse_body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_001\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-20250514\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n",
        "\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n",
        "\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
        "\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n",
        "\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n",
        "\n",
    );

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    // Create provider pointed at mock server
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

    // Verify events
    let event = stream.next().await.expect("should have Start");
    assert!(matches!(
        event,
        ai_provider::AssistantMessageEvent::Start { .. }
    ));

    let event = stream.next().await.expect("should have TextStart");
    assert!(matches!(
        event,
        ai_provider::AssistantMessageEvent::TextStart {
            content_index: 0,
            ..
        }
    ));

    let event = stream.next().await.expect("should have TextDelta 'Hello'");
    assert!(
        matches!(&event, ai_provider::AssistantMessageEvent::TextDelta { delta, .. } if delta == "Hello")
    );

    let event = stream.next().await.expect("should have TextDelta ' world'");
    assert!(
        matches!(&event, ai_provider::AssistantMessageEvent::TextDelta { delta, .. } if delta == " world")
    );

    let event = stream.next().await.expect("should have TextEnd");
    assert!(matches!(
        event,
        ai_provider::AssistantMessageEvent::TextEnd {
            content_index: 0,
            ..
        }
    ));

    let event = stream.next().await.expect("should have Done");
    match event {
        ai_provider::AssistantMessageEvent::Done { reason, .. } => {
            assert_eq!(reason, ai_provider::StopReason::Stop);
        }
        _ => panic!("expected Done"),
    }

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_anthropic_smoke_tool_use() {
    let server = MockServer::start().await;

    let sse_body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_002\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-20250514\",\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n",
        "\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_001\",\"name\":\"read\"}}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"/x\\\"}\"}}\n",
        "\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
        "\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":15}}\n",
        "\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n",
        "\n",
    );

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

    let _ = stream.next().await; // Start
    let _ = stream.next().await; // ToolCallStart
    let _ = stream.next().await; // ToolCallDelta
    let _ = stream.next().await; // ToolCallDelta

    let event = stream.next().await.expect("should have ToolCallEnd");
    match event {
        ai_provider::AssistantMessageEvent::ToolCallEnd { tool_call, .. } => {
            assert_eq!(tool_call.name, "read");
            assert_eq!(tool_call.id, "toolu_001");
            assert_eq!(tool_call.arguments["path"], "/x");
        }
        _ => panic!("expected ToolCallEnd"),
    }

    let event = stream.next().await.expect("should have Done");
    match event {
        ai_provider::AssistantMessageEvent::Done { reason, .. } => {
            assert_eq!(reason, ai_provider::StopReason::ToolUse);
        }
        _ => panic!("expected Done"),
    }
}
