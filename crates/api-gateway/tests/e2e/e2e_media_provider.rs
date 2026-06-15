//! End-to-end integration test: MediaGenerationTool with mock MediaProvider.
//!
//! Verifies that when the LLM emits `generate_media` tool_calls, the
//! MediaGenerationTool executes, calls the mock MediaProvider, and returns
//! either an inline image or a saved file path depending on size.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_core::AgentSpace;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// Mock MediaProvider that returns configurable responses.
struct MockMediaProvider {
    client: reqwest::Client,
    response: std::sync::Mutex<Option<ai_provider::media::MediaResponse>>,
}

impl MockMediaProvider {
    fn new(response: ai_provider::media::MediaResponse) -> Self {
        Self {
            client: reqwest::Client::new(),
            response: std::sync::Mutex::new(Some(response)),
        }
    }
}

#[async_trait::async_trait]
impl ai_provider::media::MediaProvider for MockMediaProvider {
    fn provider_name(&self) -> &str {
        "mock"
    }

    fn supported_tasks(&self) -> Vec<ai_provider::media::MediaTaskType> {
        vec![
            ai_provider::media::MediaTaskType::ImageGeneration,
            ai_provider::media::MediaTaskType::VideoGeneration,
            ai_provider::media::MediaTaskType::AudioGeneration,
        ]
    }

    async fn generate(
        &self,
        _model: &str,
        _request: ai_provider::media::MediaRequest,
        _signal: CancellationToken,
    ) -> Result<ai_provider::media::MediaResponse, ai_provider::media::MediaError> {
        let mut guard = self.response.lock().unwrap();
        guard.take().ok_or_else(|| {
            ai_provider::media::MediaError::TaskFailed("no more mock responses".into())
        })
    }

    fn client(&self) -> &reqwest::Client {
        &self.client
    }
}

use tokio_util::sync::CancellationToken;

async fn build_app_with_media(
    provider: Arc<dyn ai_provider::LlmProvider>,
    media_provider: Option<Arc<dyn ai_provider::media::MediaProvider>>,
    media_registry: Option<Arc<dyn ai_provider::media::MediaGenerationRegistry>>,
) -> axum::Router {
    let harness_config = agent_core::HarnessConfig {
        provider: provider.clone(),
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are a helpful assistant.".to_string(),
        default_context_window: 128_000,
        store: None,
        media_provider,
        media_registry,
        http_client: reqwest::Client::new(),
        available_models: vec!["gpt-4".to_string()],
        compaction_config: agent_core::CompactionConfig::default(),
        agent_space: agent_core::AgentSpace::default(),
        hook_config: agent_core::HarnessConfig::default().hook_config,
        memory_store: None,
    };
    common::build_test_app_with_config(provider, harness_config).await
}
/// SSE body that emits a `generate_media` tool call.
fn generate_media_sse_body(media_type: &str, prompt: &str) -> String {
    let args = serde_json::json!({"media_type": media_type, "prompt": prompt}).to_string();
    format!(
        r#"data: {{"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{{"delta":{{"role":"assistant"}},"index":0}}]}}

data: {{"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{{"delta":{{"tool_calls":[{{"index":0,"id":"call_media1","function":{{"name":"generate_media"}}}}]}},"index":0}}]}}

data: {{"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{{"delta":{{"tool_calls":[{{"index":0,"function":{{"arguments":"{}"}}}}]}},"index":0}}]}}

data: {{"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{{"delta":{{}},"finish_reason":"tool_calls","index":0}}]}}

data: [DONE]

"#,
        args.replace('"', "\\\"")
    )
}

fn text_after_tool_sse_body(text: &str) -> String {
    format!(
        r#"data: {{"id":"chatcmpl-t2","object":"chat.completion.chunk","choices":[{{"delta":{{"role":"assistant"}},"index":0}}]}}

data: {{"id":"chatcmpl-t2","object":"chat.completion.chunk","choices":[{{"delta":{{"content":"{}"}},"index":0}}]}}

data: {{"id":"chatcmpl-t2","object":"chat.completion.chunk","choices":[{{"delta":{{}},"finish_reason":"stop","index":0}}]}}

data: [DONE]

"#,
        text
    )
}

#[tokio::test]
async fn test_media_generation_returns_inline_image() {
    let _ = tracing_subscriber::fmt().try_init();

    // Small base64 PNG (1x1 transparent pixel)
    let small_png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let media_response = ai_provider::media::MediaResponse::Inline {
        data: small_png.to_string(),
        mime_type: "image/png".to_string(),
    };

    let turn1 = generate_media_sse_body("image", "a cat");
    let turn2 = text_after_tool_sse_body("here is your image");

    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();
    let responder = move |_req: &wiremock::Request| {
        let c = cc.fetch_add(1, Ordering::SeqCst);
        if c == 0 {
            wiremock::ResponseTemplate::new(200).set_body_string(&turn1)
        } else {
            wiremock::ResponseTemplate::new(200).set_body_string(&turn2)
        }
    };

    let (_server, provider) = common::start_wiremock_openai_dynamic(responder).await;
    let media_provider: Arc<dyn ai_provider::media::MediaProvider> =
        Arc::new(MockMediaProvider::new(media_response));
    let app = build_app_with_media(provider, media_provider).await;
    let token = "pk_live_test-tenant";

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "media test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let sid = common::json_body(create).await["id"].as_str().unwrap().to_string();

    let sse_app = app.clone();
    let sse_token = token.clone();
    let sse_sid = sid.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let sse_handle = tokio::spawn(async move {
        let resp = sse_app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/sessions/{}/events", sse_sid))
                    .header("Authorization", format!("Bearer {}", sse_token))
                    .header("Accept", "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let _ = ready_tx.send(());
        common::collect_sse_events(resp).await
    });
    ready_rx.await.unwrap();

    let send = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"content": [{"type":"text","text":"draw a cat"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send.status(), StatusCode::OK);

    let events = tokio::time::timeout(std::time::Duration::from_secs(5), sse_handle)
        .await
        .unwrap()
        .unwrap();

    let tool_done = events
        .iter()
        .find(|e| matches!(e, api_gateway::types::ServerEvent::ToolCallDone { .. }));
    assert!(tool_done.is_some(), "expected ToolCallDone");

    if let api_gateway::types::ServerEvent::ToolCallDone { is_error, .. } = tool_done.unwrap() {
        assert!(!is_error, "media generation should succeed");
    }

    // Verify ToolCallDone result contains image marker (downgraded in /messages endpoint)
    if let api_gateway::types::ServerEvent::ToolCallDone { result, .. } = tool_done.unwrap() {
        let result_text = result.as_deref().unwrap_or("");
        assert!(
            result_text.contains("[image:") || result_text.contains("image"),
            "tool result should reference image, got: {:?}",
            result
        );
    }
}

#[tokio::test]
async fn test_media_generation_saves_large_image_to_workspace() {
    let _ = tracing_subscriber::fmt().try_init();

    // Large base64 data (> 1MB raw after decode ≈ > 1.33MB base64 string)
    // Generate ~2MB of base64 data
    let large_bytes = vec![0u8; 2 * 1024 * 1024];
    let large_base64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &large_bytes,
    );

    let media_response = ai_provider::media::MediaResponse::Inline {
        data: large_base64,
        mime_type: "image/png".to_string(),
    };

    let turn1 = generate_media_sse_body("image", "a huge cat");
    let turn2 = text_after_tool_sse_body("saved");

    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();
    let responder = move |_req: &wiremock::Request| {
        let c = cc.fetch_add(1, Ordering::SeqCst);
        if c == 0 {
            wiremock::ResponseTemplate::new(200).set_body_string(&turn1)
        } else {
            wiremock::ResponseTemplate::new(200).set_body_string(&turn2)
        }
    };

    let (_server, provider) = common::start_wiremock_openai_dynamic(responder).await;
    let media_provider: Arc<dyn ai_provider::media::MediaProvider> =
        Arc::new(MockMediaProvider::new(media_response));
    let app = build_app_with_media(provider, media_provider).await;
    let token = "pk_live_test-tenant";

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "media large"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = common::json_body(create).await["id"].as_str().unwrap().to_string();

    let sse_app = app.clone();
    let sse_token = token.clone();
    let sse_sid = sid.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let sse_handle = tokio::spawn(async move {
        let resp = sse_app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/sessions/{}/events", sse_sid))
                    .header("Authorization", format!("Bearer {}", sse_token))
                    .header("Accept", "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let _ = ready_tx.send(());
        common::collect_sse_events(resp).await
    });
    ready_rx.await.unwrap();

    let send = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"content": [{"type":"text","text":"draw a huge cat"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send.status(), StatusCode::OK);

    let events = tokio::time::timeout(std::time::Duration::from_secs(5), sse_handle)
        .await
        .unwrap()
        .unwrap();

    let tool_done = events
        .iter()
        .find(|e| matches!(e, api_gateway::types::ServerEvent::ToolCallDone { .. }));
    assert!(tool_done.is_some());

    if let api_gateway::types::ServerEvent::ToolCallDone { is_error, .. } = tool_done.unwrap() {
        assert!(!is_error, "large media save should succeed");
    }

    // Verify message history contains file path
    let msgs_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let msgs = common::json_body(msgs_response).await;
    let has_path = msgs.as_array().unwrap().iter().any(|m| {
        m.get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter().any(|item| {
                    item.get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| s.contains("已保存至"))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    });
    assert!(has_path, "message history should contain saved file path");
}
