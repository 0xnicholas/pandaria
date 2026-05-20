//! End-to-end transform integration tests: verify that AgentLoop applies
//! `ai_provider::transform_messages` before sending the HTTP request body.
//!
//! We inject messages containing images and thinking blocks into the session,
//! then inspect the actual JSON body received by the wiremock server.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_core::{
    CompactionActor, CompactionConfig, DefaultFileOperationExtractor, SessionActor, SessionConfig,
};
use agent_core::test_utils::AllowAllDispatcher;
use agent_core::types::AgentMessage;
use ai_provider::{
    AssistantMessage, Content, LlmProvider, Message, StopReason, Usage,
    providers::{anthropic::AnthropicProvider, openai::OpenAiProvider},
};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_compaction_actor(provider: Arc<dyn LlmProvider>) -> Arc<CompactionActor> {
    Arc::new(CompactionActor::new(
        CompactionConfig::default(),
        provider,
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    ))
}

/// Build a user message that contains both text and an image.
fn user_msg_with_image() -> Message {
    Message::User(ai_provider::UserMessage {
        content: vec![
            Content::Text {
                text: "Describe this image".to_string(),
                text_signature: None,
            },
            Content::Image {
                data: "fakebase64".to_string(),
                mime_type: "image/png".to_string(),
            },
        ],
        timestamp: std::time::SystemTime::now(),
    })
}

/// Build an assistant message that contains both thinking and text.
fn assistant_msg_with_thinking() -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![
            Content::Thinking {
                thinking: "I need to analyze the image carefully".to_string(),
                thinking_signature: None,
                redacted: false,
            },
            Content::Text {
                text: "It looks like a cat.".to_string(),
                text_signature: None,
            },
        ],
        provider: "test".to_string(),
        model: "test".to_string(),
        api: ai_provider::Api {
            provider: "test".to_string(),
            model: "test".to_string(),
        },
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            total_tokens: 15,
        },
        stop_reason: StopReason::Stop,
        response_id: None,
        error_message: None,
        timestamp: std::time::SystemTime::now(),
    })
}

// ============================================================================
// OpenAI path: image downgrade + thinking removal
// ============================================================================

#[tokio::test]
async fn test_openai_request_body_has_transformed_messages() {
    let _ = tracing_subscriber::fmt().try_init();

    let server = MockServer::start().await;
    let body_valid = Arc::new(AtomicBool::new(false));
    let body_valid_clone = body_valid.clone();

    let sse_body = r#"data: {"id":"chatcmpl-tx","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-tx","object":"chat.completion.chunk","choices":[{"delta":{"content":"ok"},"index":0}]}

data: {"id":"chatcmpl-tx","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let messages = body["messages"].as_array().unwrap();

            // messages[0] should be the user message with image downgraded
            let user_msg = messages.iter().find(|m| m["role"] == "user").unwrap();
            let content = user_msg["content"].as_array().unwrap();
            // Image should be downgraded to a text placeholder
            let has_image_placeholder = content.iter().any(|c| {
                c["type"] == "text"
                    && c["text"]
                        .as_str()
                        .unwrap_or("")
                        .contains("image omitted")
            });
            assert!(
                has_image_placeholder,
                "image should be downgraded to placeholder in OpenAI request"
            );

            // Find the assistant message with thinking removed
            let assist_msg = messages.iter().find(|m| m["role"] == "assistant").unwrap();
            let assist_content = assist_msg["content"].as_array().unwrap();
            let has_thinking = assist_content.iter().any(|c| c["type"] == "thinking");
            assert!(
                !has_thinking,
                "thinking block should be removed from assistant message"
            );
            let has_text = assist_content.iter().any(|c| {
                c["type"] == "text" && c["text"].as_str().unwrap_or("") == "It looks like a cat."
            });
            assert!(has_text, "text content should be preserved");

            body_valid_clone.store(true, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_string(sse_body)
        })
        .mount(&server)
        .await;

    let provider: Arc<dyn LlmProvider> = Arc::new(
        OpenAiProvider::with_base_url(
            Some(SecretString::new("sk-test".into())),
            &server.uri(),
        )
    );

    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        // Use a model name NOT in the registry so supports_images = false
        model: "test-no-image".to_string(),
        provider: provider.clone(),
        hook_dispatcher: Arc::new(AllowAllDispatcher),
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    session.push_message(AgentMessage::from(user_msg_with_image()));
    session.push_message(AgentMessage::from(assistant_msg_with_thinking()));

    let result: String = session.complete("What do you see?".to_string()).await.unwrap();
    assert_eq!(result, "ok");
    assert!(body_valid.load(Ordering::SeqCst), "request body assertions should have run");
}

// ============================================================================
// Anthropic path: image downgrade + thinking removal
// ============================================================================

#[tokio::test]
async fn test_anthropic_request_body_has_transformed_messages() {
    let _ = tracing_subscriber::fmt().try_init();

    let server = MockServer::start().await;
    let body_valid = Arc::new(AtomicBool::new(false));
    let body_valid_clone = body_valid.clone();

    let sse_body = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_tx","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","usage":{"input_tokens":5,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}

event: message_stop
data: {"type":"message_stop"}

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let messages = body["messages"].as_array().unwrap();

            // messages[0] = user with image downgraded
            let user_msg = messages.iter().find(|m| m["role"] == "user").unwrap();
            let content = user_msg["content"].as_array().unwrap();
            let has_image_placeholder = content.iter().any(|c| {
                c["type"] == "text"
                    && c["text"]
                        .as_str()
                        .unwrap_or("")
                        .contains("image omitted")
            });
            assert!(
                has_image_placeholder,
                "image should be downgraded to placeholder in Anthropic request"
            );

            // Find the assistant message with thinking removed
            let assist_msg = messages.iter().find(|m| m["role"] == "assistant").unwrap();
            let assist_content = assist_msg["content"].as_array().unwrap();
            let has_thinking = assist_content.iter().any(|c| c["type"] == "thinking");
            assert!(
                !has_thinking,
                "thinking block should be removed from assistant message"
            );
            let has_text = assist_content.iter().any(|c| {
                c["type"] == "text" && c["text"].as_str().unwrap_or("") == "It looks like a cat."
            });
            assert!(has_text, "text content should be preserved");

            body_valid_clone.store(true, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_string(sse_body)
        })
        .mount(&server)
        .await;

    let provider: Arc<dyn LlmProvider> = Arc::new(
        AnthropicProvider::with_base_url(
            Some(SecretString::new("sk-test".into())),
            &server.uri(),
        )
    );

    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s2".to_string(),
        system_prompt: "You are helpful.".to_string(),
        // Use a model name NOT in the registry so supports_images = false
        model: "test-no-image".to_string(),
        provider: provider.clone(),
        hook_dispatcher: Arc::new(AllowAllDispatcher),
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    session.push_message(AgentMessage::from(user_msg_with_image()));
    session.push_message(AgentMessage::from(assistant_msg_with_thinking()));

    let result: String = session.complete("What do you see?".to_string()).await.unwrap();
    assert_eq!(result, "ok");
    assert!(body_valid.load(Ordering::SeqCst), "request body assertions should have run");
}

// ============================================================================
// Vision model path: image preserved
// ============================================================================

#[tokio::test]
async fn test_vision_model_preserves_image() {
    let _ = tracing_subscriber::fmt().try_init();

    let server = MockServer::start().await;
    let body_valid = Arc::new(AtomicBool::new(false));
    let body_valid_clone = body_valid.clone();

    let sse_body = r#"data: {"id":"chatcmpl-v","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-v","object":"chat.completion.chunk","choices":[{"delta":{"content":"ok"},"index":0}]}

data: {"id":"chatcmpl-v","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let messages = body["messages"].as_array().unwrap();
            let user_msg = messages.iter().find(|m| m["role"] == "user").unwrap();
            let content = user_msg["content"].as_array().unwrap();
            let has_image = content.iter().any(|c| c["type"] == "image_url");
            assert!(
                has_image,
                "image_url should be preserved for vision-capable model"
            );

            body_valid_clone.store(true, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_string(sse_body)
        })
        .mount(&server)
        .await;

    let provider: Arc<dyn LlmProvider> = Arc::new(
        OpenAiProvider::with_base_url(
            Some(SecretString::new("sk-test".into())),
            &server.uri(),
        )
    );

    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s3".to_string(),
        system_prompt: "You are helpful.".to_string(),
        // gpt-5.2 is in the registry and supports images
        model: "gpt-5.2".to_string(),
        provider: provider.clone(),
        hook_dispatcher: Arc::new(AllowAllDispatcher),
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    session.push_message(AgentMessage::from(user_msg_with_image()));

    let result: String = session.complete("Describe it".to_string()).await.unwrap();
    assert_eq!(result, "ok");
    assert!(body_valid.load(Ordering::SeqCst), "request body assertions should have run");
}
