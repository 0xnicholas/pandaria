//! Mistral provider integration tests via wiremock.

use ai_provider::providers::mistral::MistralProvider;
use ai_provider::{
    AssistantMessageEvent, Content, LlmContext, LlmProvider, StopReason, StreamOptions, ToolCall,
};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_mock_basic_text_stream() {
    let server = MockServer::start().await;

    let sse_body = r#"data: {"id":"cmpl-1","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"cmpl-1","object":"chat.completion.chunk","choices":[{"delta":{"content":"Bonjour"},"index":0}]}

data: {"id":"cmpl-1","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
        .mount(&server)
        .await;

    let provider =
        MistralProvider::with_base_url(Some(SecretString::new("sk-test".into())), &server.uri());

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut stream = provider
        .stream(
            "mistral-large-latest",
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

    let event = stream.next().await.expect("should have TextDelta");
    assert!(
        matches!(&event, AssistantMessageEvent::TextDelta { delta, content_index: 0, .. } if delta == "Bonjour")
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
async fn test_tool_call_id_truncation_in_request_body() {
    let server = MockServer::start().await;

    let long_id = "a".repeat(50);
    let body_had_truncated_id = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let body_had_truncated_id_clone = body_had_truncated_id.clone();

    let sse_body = r#"data: {"id":"cmpl-2","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"cmpl-2","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let messages = body["messages"].as_array().unwrap();
            let assist_msg = messages.iter().find(|m| m["role"] == "assistant").unwrap();
            let content = assist_msg["content"].as_array().unwrap();
            let tc_id = content
                .iter()
                .find_map(|c| c.get("id").and_then(|v| v.as_str()))
                .unwrap_or("");
            if tc_id.len() <= 36 {
                body_had_truncated_id_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            ResponseTemplate::new(200).set_body_string(sse_body)
        })
        .mount(&server)
        .await;

    let provider =
        MistralProvider::with_base_url(Some(SecretString::new("sk-test".into())), &server.uri());

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![ai_provider::Message::Assistant(
            ai_provider::AssistantMessage {
                content: vec![Content::ToolCall(ToolCall {
                    id: long_id.clone(),
                    name: "test".to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                })],
                provider: "test".to_string(),
                model: "test".to_string(),
                api: ai_provider::Api {
                    provider: "test".to_string(),
                    model: "test".to_string(),
                },
                usage: ai_provider::Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: StopReason::ToolUse,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            },
        )],
        tools: None,
    };

    let mut stream = provider
        .stream(
            "mistral-large-latest",
            ctx,
            StreamOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("stream should start");

    while stream.next().await.is_some() {}

    assert!(
        body_had_truncated_id.load(std::sync::atomic::Ordering::SeqCst),
        "Mistral should truncate tool call IDs to <= 36 chars in request body"
    );
}

#[tokio::test]
async fn test_reasoning_request_body_has_prompt_mode() {
    let server = MockServer::start().await;

    let body_had_prompt_mode = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let body_had_prompt_mode_clone = body_had_prompt_mode.clone();

    let sse_body = r#"data: {"id":"cmpl-3","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"cmpl-3","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            if body["promptMode"] == "reasoning" && body["reasoningEffort"] == "high" {
                body_had_prompt_mode_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            ResponseTemplate::new(200).set_body_string(sse_body)
        })
        .mount(&server)
        .await;

    let provider =
        MistralProvider::with_base_url(Some(SecretString::new("sk-test".into())), &server.uri());

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![],
        tools: None,
    };

    let mut opts = StreamOptions::default();
    opts.reasoning = Some(ai_provider::ReasoningLevel::High);

    let mut stream = provider
        .stream("mistral-large-latest", ctx, opts, CancellationToken::new())
        .await
        .expect("stream should start");

    while stream.next().await.is_some() {}

    assert!(
        body_had_prompt_mode.load(std::sync::atomic::Ordering::SeqCst),
        "Mistral should include promptMode=reasoning and reasoningEffort in request body"
    );
}
