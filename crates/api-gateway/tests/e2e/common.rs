//! Shared helpers for api-gateway end-to-end integration tests.
//!
//! These tests use the *real* `TenantManagerImpl` (not mocks) backed by
//! a `wiremock` LLM server, so they exercise the full stack:
//! HTTP → auth → tenant → agent-core → ai-provider.

use std::sync::Arc;

use api_gateway::{AppState, ServerConfig};
use axum::Router;
use axum::http::StatusCode;
use axum::response::Response;

/// Build a test router with a real `TenantManagerImpl` and the given provider.
pub fn build_test_app(provider: Arc<dyn ai_provider::LlmProvider>) -> Router {
    let registry = Arc::new(tenant::TenantRegistry::new());
    let test_tenant = tenant::Tenant::new(
        "test-tenant",
        tenant::TenantQuota {
            max_concurrent_sessions: 10,
            max_tokens_per_day: 1_000_000,
            max_tool_calls_per_minute: 60,
            cpu_time_budget_ms_per_day: 3_600_000,
        },
    );
    registry.register(test_tenant).unwrap();

    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(
            registry,
            provider,
            None, // no persistent store
            "gpt-4",
            "You are a helpful assistant.",
            128_000,
        ),
    );

    let config = ServerConfig {
        auth_secret: secrecy::SecretString::from(TEST_SECRET),
        ..Default::default()
    };
    let state = Arc::new(AppState::new(manager, config));
    api_gateway::build_router(state)
}

/// Build a test router with a custom tenant registry.
pub fn build_test_app_with_registry(
    provider: Arc<dyn ai_provider::LlmProvider>,
    registry: Arc<tenant::TenantRegistry>,
) -> Router {
    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(
            registry,
            provider,
            None,
            "gpt-4",
            "You are a helpful assistant.",
            128_000,
        ),
    );

    let config = ServerConfig {
        auth_secret: secrecy::SecretString::from(TEST_SECRET),
        ..Default::default()
    };
    let state = Arc::new(AppState::new(manager, config));
    api_gateway::build_router(state)
}

const TEST_SECRET: &str = "test-secret-32-chars-long!!!";

/// Generate a valid HMAC-SHA256 token for the given tenant.
pub fn make_token(tenant_id: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let payload = serde_json::json!({
        "tenant_id": tenant_id,
        "iat": now,
        "exp": now + 86400,
    });
    let payload_json = serde_json::to_vec(&payload).unwrap();
    let payload_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        &payload_json,
    );

    let mut mac = Hmac::<Sha256>::new_from_slice(TEST_SECRET.as_bytes()).unwrap();
    mac.update(&payload_json);
    let signature = mac.finalize().into_bytes();
    let sig_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        &signature,
    );

    format!("{}.{}", payload_b64, sig_b64)
}

/// Start a wiremock server that responds with the given SSE body for OpenAI
/// chat completions, and return the provider + base URL.
pub async fn start_wiremock_openai(body: &str) -> (wiremock::MockServer, Arc<dyn ai_provider::LlmProvider>) {
    let server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let provider: Arc<dyn ai_provider::LlmProvider> = Arc::new(
        ai_provider::providers::openai::OpenAiProvider::with_base_url(
            Some(secrecy::SecretString::from("sk-test")),
            &server.uri(),
        )
    );

    (server, provider)
}

/// Start a wiremock server with a dynamic responder and return the provider.
pub async fn start_wiremock_openai_dynamic<F>(responder: F) -> (wiremock::MockServer, Arc<dyn ai_provider::LlmProvider>)
where
    F: Fn(&wiremock::Request) -> wiremock::ResponseTemplate + Send + Sync + 'static,
{
    let server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/"))
        .respond_with(responder)
        .mount(&server)
        .await;

    let provider: Arc<dyn ai_provider::LlmProvider> = Arc::new(
        ai_provider::providers::openai::OpenAiProvider::with_base_url(
            Some(secrecy::SecretString::from("sk-test")),
            &server.uri(),
        )
    );

    (server, provider)
}

/// Parse an HTTP response body into a JSON value.
/// Panics if the status code is not success (2xx).
pub async fn json_body(response: Response) -> serde_json::Value {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        status.is_success(),
        "expected success status, got {}: {}",
        status,
        value
    );
    value
}

/// Collect SSE events from an HTTP response body with a timeout.
///
/// Parses the `event: ...\ndata: ...\n\n` format produced by `SseStream`.
/// Because SSE streams may be kept alive indefinitely, this function reads
/// until `timeout` expires and then returns whatever events were captured.
pub async fn collect_sse_events(response: Response) -> Vec<api_gateway::types::ServerEvent> {
    collect_sse_events_with_timeout(response, std::time::Duration::from_secs(10)).await
}

pub async fn collect_sse_events_with_timeout(
    response: Response,
    timeout: std::time::Duration,
) -> Vec<api_gateway::types::ServerEvent> {
    use futures::StreamExt;
    use tokio::time::{timeout as tokio_timeout, Instant};

    let mut body_stream = response.into_body().into_data_stream();
    let mut buffer = String::new();
    let mut events = Vec::new();

    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        let remaining = deadline.duration_since(Instant::now());
        match tokio_timeout(remaining, body_stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                if let Ok(text) = std::str::from_utf8(&chunk) {
                    buffer.push_str(text);
                }

                // Parse SSE events from buffer (separated by double-newline)
                while let Some(pos) = buffer.find("\n\n") {
                    let event_text = buffer[..pos].to_string();
                    buffer = buffer[pos + 2..].to_string();

                    for line in event_text.lines() {
                        if line.starts_with("data: ") {
                            let json = &line[6..];
                            if let Ok(event) =
                                serde_json::from_str::<api_gateway::types::ServerEvent>(json)
                            {
                                let is_end =
                                    matches!(event, api_gateway::types::ServerEvent::TurnEnd { .. });
                                events.push(event);
                                if is_end {
                                    return events;
                                }
                            }
                        }
                    }
                }
            }
            Ok(Some(Err(e))) => {
                panic!("body stream error: {e}");
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    events
}

/// Standard OpenAI SSE body for a simple text response.
pub fn openai_text_sse_body(text: &str) -> String {
    format!(
        r#"data: {{"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{{"delta":{{"role":"assistant"}},"index":0}}]}}

data: {{"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{{"delta":{{"content":"{}"}},"index":0}}]}}

data: {{"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{{"delta":{{}},"finish_reason":"stop","index":0}}]}}

data: [DONE]

"#,
        text
    )
}
