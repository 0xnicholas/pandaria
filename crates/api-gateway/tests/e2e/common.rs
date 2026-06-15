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

    let runtime_config = Arc::new(agent_core::HarnessConfig {
        provider: provider.clone(),
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are a helpful assistant.".to_string(),
        default_context_window: 128_000,
        store: None,
        media_provider: None,
        media_registry: None,
        http_client: reqwest::Client::new(),
        available_models: vec!["gpt-4".to_string()],
        compaction_config: agent_core::CompactionConfig::default(),
        agent_space: agent_core::AgentSpace::default(),
        hook_config: agent_core::HookConfig::default(),
        memory_store: None,
    });
    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(registry, runtime_config),
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
    let runtime_config = Arc::new(agent_core::HarnessConfig {
        provider: provider.clone(),
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are a helpful assistant.".to_string(),
        default_context_window: 128_000,
        store: None,
        media_provider: None,
        media_registry: None,
        http_client: reqwest::Client::new(),
        available_models: vec!["gpt-4".to_string()],
        compaction_config: agent_core::CompactionConfig::default(),
        agent_space: agent_core::AgentSpace::default(),
        hook_config: agent_core::HookConfig::default(),
        memory_store: None,
    });
    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(registry, runtime_config),
    );

    let config = ServerConfig {
        auth_secret: secrecy::SecretString::from(TEST_SECRET),
        ..Default::default()
    };
    let state = Arc::new(AppState::new(manager, config));
    api_gateway::build_router(state)
}

/// Build a test router with a custom HTTP client for external tools/webhooks.
///
/// The `client` is injected into `TenantManagerImpl` so that `HttpProxyTool`
/// and `WebhookEventListener` use it for outbound requests. This allows E2E
/// tests to route requests to local mock servers (e.g. wiremock) while using
/// a public-looking domain that passes SSRF checks.
pub fn build_test_app_with_client(
    provider: Arc<dyn ai_provider::LlmProvider>,
    client: reqwest::Client,
) -> Router {
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

    let runtime_config = Arc::new(agent_core::HarnessConfig {
        provider: provider.clone(),
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are a helpful assistant.".to_string(),
        default_context_window: 128_000,
        store: None,
        media_provider: None,
        media_registry: None,
        http_client: client,
        available_models: vec!["gpt-4".to_string()],
        compaction_config: agent_core::CompactionConfig::default(),
        agent_space: agent_core::AgentSpace::default(),
        hook_config: agent_core::HookConfig::default(),
        memory_store: None,
    });
    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(registry, runtime_config),
    );

    let config = ServerConfig {
        auth_secret: secrecy::SecretString::from(TEST_SECRET),
        ..Default::default()
    };
    let state = Arc::new(AppState::new(manager, config));
    api_gateway::build_router(state)
}

pub const TEST_SECRET: &str = "test-secret-32-chars-long!!!";

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
pub async fn start_wiremock_openai(
    body: &str,
) -> (wiremock::MockServer, Arc<dyn ai_provider::LlmProvider>) {
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
        ),
    );

    (server, provider)
}

/// Start a wiremock server with a dynamic responder and return the provider.
pub async fn start_wiremock_openai_dynamic<F>(
    responder: F,
) -> (wiremock::MockServer, Arc<dyn ai_provider::LlmProvider>)
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
        ),
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
    use tokio::time::{Instant, timeout as tokio_timeout};

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
                                let is_end = matches!(
                                    event,
                                    api_gateway::types::ServerEvent::TurnEnd { .. }
                                );
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

// ── Persistence-aware test fixtures ──

use storage::session::postgres::PgSessionStore;

/// Build a test router with both a real TenantManagerImpl and a SessionStore.
/// Delegates to `build_test_app_with_store_and_compaction` with default config.
pub fn build_test_app_with_store(
    provider: Arc<dyn ai_provider::LlmProvider>,
    store: Arc<dyn agent_core::SessionStore>,
) -> Router {
    build_test_app_with_store_and_compaction(
        provider,
        store,
        agent_core::CompactionConfig::default(),
    )
}

/// Build a test router with a SessionStore and custom CompactionConfig.
pub fn build_test_app_with_store_and_compaction(
    provider: Arc<dyn ai_provider::LlmProvider>,
    store: Arc<dyn agent_core::SessionStore>,
    compaction_config: agent_core::CompactionConfig,
) -> Router {
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

    let runtime_config = Arc::new(agent_core::HarnessConfig {
        provider: provider.clone(),
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are a helpful assistant.".to_string(),
        default_context_window: 128_000,
        store: Some(store),
        media_provider: None,
        media_registry: None,
        http_client: reqwest::Client::new(),
        available_models: vec!["gpt-4".to_string()],
        compaction_config,
        agent_space: agent_core::AgentSpace::default(),
        hook_config: agent_core::HookConfig::default(),
        memory_store: None,
    });
    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(registry, runtime_config),
    );

    let config = ServerConfig {
        auth_secret: secrecy::SecretString::from(TEST_SECRET),
        ..Default::default()
    };
    let state = Arc::new(AppState::new(manager, config));
    api_gateway::build_router(state)
}

/// Verify Docker containers are running before persistence-dependent tests.
pub async fn ensure_test_containers() {
    let pg_url = std::env::var("PANDARIA_TEST_PG_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:15432/postgres".to_string());
    let redis_url = std::env::var("PANDARIA_TEST_REDIS_URL")
        .unwrap_or_else(|_| "redis://:redis@localhost:16379".to_string());

    let pg_ok = sqlx::PgPool::connect(&pg_url).await.is_ok();
    // Simple connectivity check: try to open a client and get connection
    let redis_ok = redis::Client::open(redis_url.as_str())
        .map(|_| ())
        .is_ok();

    if !pg_ok || !redis_ok {
        panic!(
            "测试容器未启动。请先运行:\n\
             docker start docker-env-postgres docker-env-redis\n\
             或设置环境变量 PANDARIA_TEST_PG_URL / PANDARIA_TEST_REDIS_URL"
        );
    }
}

/// Create a PgSessionStore connected to the test PostgreSQL container.
pub async fn create_test_pg_store() -> PgSessionStore {
    let url = std::env::var("PANDARIA_TEST_PG_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:15432/postgres".to_string());
    let pool = sqlx::PgPool::connect(&url)
        .await
        .expect("failed to connect to test postgres");
    let store = PgSessionStore::new(pool);
    store.init().await.expect("pg init failed");
    store
}

// ── Aspectus Integration Test Helpers ──

#[cfg(feature = "aspectus-auth")]
/// Mock Aspectus server backed by wiremock.
pub struct AspectusMock {
    pub server: wiremock::MockServer,
}

#[cfg(feature = "aspectus-auth")]
impl AspectusMock {
    pub async fn start() -> Self {
        Self {
            server: wiremock::MockServer::start().await,
        }
    }

    pub fn base_url(&self) -> String {
        self.server.uri()
    }

    /// Mock a successful introspection response for a tenant.
    pub async fn mock_active_tenant(&self, tenant_id: &str) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/introspect"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({
                    "active": true,
                    "tenant_id": tenant_id,
                    "user_id": "test-user",
                    "scope": "pandaria:session:create pandaria:session:read",
                    "quotas": {
                        "pandaria": {
                            "max_concurrent_sessions": 10,
                            "max_tokens_per_day": 1000000,
                            "max_tool_calls_per_minute": 60,
                            "cpu_time_budget_ms_per_day": 3600000
                        }
                    }
                }),
            ))
            .mount(&self.server)
            .await;
    }

    /// Mock a successful introspection with custom quota (for testing limits).
    pub async fn mock_tenant_with_quota(
        &self,
        tenant_id: &str,
        max_sessions: u32,
    ) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/introspect"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({
                    "active": true,
                    "tenant_id": tenant_id,
                    "user_id": "test-user",
                    "scope": "pandaria:session:create pandaria:session:read",
                    "quotas": {
                        "pandaria": {
                            "max_concurrent_sessions": max_sessions,
                            "max_tokens_per_day": 1000000,
                            "max_tool_calls_per_minute": 60,
                            "cpu_time_budget_ms_per_day": 3600000
                        }
                    }
                }),
            ))
            .mount(&self.server)
            .await;
    }

    /// Mock an inactive token response.
    pub async fn mock_inactive(&self) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/introspect"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"active": false})),
            )
            .mount(&self.server)
            .await;
    }

    /// Mock a tenant not configured for pandaria (no quotas.pandaria key).
    pub async fn mock_no_pandaria_quota(&self, tenant_id: &str) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/introspect"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({
                    "active": true,
                    "tenant_id": tenant_id,
                    "user_id": "test-user",
                    "scope": "pandaria:session:create",
                }),
            ))
            .mount(&self.server)
            .await;
    }

    /// Mock an internal server error from Aspectus (503 simulation).
    pub async fn mock_server_error(&self) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/introspect"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&self.server)
            .await;
    }
}

#[cfg(feature = "aspectus-auth")]
/// Build a test app with Aspectus auth enabled.
/// Uses real AspectusClient pointed at a wiremock server.
pub async fn build_test_app_with_aspectus(
    provider: Arc<dyn ai_provider::LlmProvider>,
    aspectus_url: String,
) -> axum::Router {
    let registry = Arc::new(tenant::TenantRegistry::new());

    let runtime_config = Arc::new(agent_core::HarnessConfig {
        provider,
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are a helpful assistant.".to_string(),
        default_context_window: 128_000,
        store: None,
        media_provider: None,
        media_registry: None,
        http_client: reqwest::Client::new(),
        available_models: vec!["gpt-4".to_string()],
        compaction_config: agent_core::CompactionConfig::default(),
        agent_space: agent_core::AgentSpace::default(),
        hook_config: agent_core::HookConfig::default(),
        memory_store: None,
    });
    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(registry, runtime_config),
    );

    let config = api_gateway::ServerConfig::default();
    let aspectus_config = api_gateway::config::AspectusConfig {
        base_url: aspectus_url,
        service_token: "test-service-token".into(),
        timeout_ms: 2000,
    };
    let state = Arc::new(
        api_gateway::AppState::with_aspectus(
            manager,
            config,
            &aspectus_config,
        )
        .expect("build test app state"),
    );

    api_gateway::build_router(state)
}
