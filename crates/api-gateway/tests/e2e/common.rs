//! Shared helpers for api-gateway end-to-end integration tests.
//!
//! These tests use the *real* `TenantManagerImpl` (not mocks) backed by
//! wiremock servers (LLM + Aspectus), so they exercise the full stack:
//! HTTP → Aspectus auth → tenant → agent-core → ai-provider.

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::response::Response;

// ── Primary builders (async, Aspectus-backed) ──

/// Build a test router with a real `TenantManagerImpl` and the given provider.
/// Starts a wiremock Aspectus server automatically with "test-tenant" configured.
pub async fn build_test_app(provider: Arc<dyn ai_provider::LlmProvider>) -> Router {
    let aspectus = AspectusMock::start().await;
    aspectus.mock_active_tenant("test-tenant").await;
    build_test_app_with_aspectus(provider, &aspectus).await
}

/// Build a test router with a custom HTTP client for external tools/webhooks.
pub async fn build_test_app_with_client(
    provider: Arc<dyn ai_provider::LlmProvider>,
    client: reqwest::Client,
) -> Router {
    let aspectus = AspectusMock::start().await;
    aspectus.mock_active_tenant("test-tenant").await;
    build_test_app_with_aspectus_and_client(provider, client, &aspectus).await
}

/// Build a test router with both a real TenantManagerImpl and a SessionStore.
pub async fn build_test_app_with_store(
    provider: Arc<dyn ai_provider::LlmProvider>,
    store: Arc<dyn agent_core::SessionStore>,
) -> Router {
    build_test_app_with_store_and_compaction(
        provider,
        store,
        agent_core::CompactionConfig::default(),
    )
    .await
}

/// Build a test router with a SessionStore and custom CompactionConfig.
pub async fn build_test_app_with_store_and_compaction(
    provider: Arc<dyn ai_provider::LlmProvider>,
    store: Arc<dyn agent_core::SessionStore>,
    compaction_config: agent_core::CompactionConfig,
) -> Router {
    let aspectus = AspectusMock::start().await;
    aspectus.mock_active_tenant("test-tenant").await;

    let registry = Arc::new(tenant::TenantRegistry::new());

    let runtime_config = Arc::new(agent_core::HarnessConfig {
        provider: provider.clone(),
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are a helpful assistant.".to_string(),
        default_context_window: 128_000,
        store: Some(store),
        media_provider: None,
        media_registry: None,
        http_client: reqwest::Client::new(),
        ssrf_policy: Arc::new(agent_core::utils::ssrf::SsrfPolicy::strict()),
        available_models: vec!["gpt-4".to_string()],
        compaction_config,
        agent_space: agent_core::AgentSpace::default(),
        hook_config: agent_core::HookConfig::default(),
        memory_store: None,
        session_retention_days: 7,
        session_cleanup_interval_hours: 24,
    });
    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(registry.clone(), runtime_config, None),
    );

    let config = api_gateway::ServerConfig::default();
    let aspectus_config = api_gateway::config::AspectusConfig {
        base_url: aspectus.base_url(),
        service_token: "test-service-token".into(),
        timeout_ms: 2000,
    };
    let state = Arc::new(
        api_gateway::AppState::new(manager, config, registry, &aspectus_config)
            .expect("build test app state"),
    );

    api_gateway::build_router(state)
}

// ── Custom-config builders ──

/// Build a test app with fully custom HarnessConfig.
pub async fn build_test_app_with_config(
    provider: Arc<dyn ai_provider::LlmProvider>,
    harness_config: agent_core::HarnessConfig,
) -> Router {
    let aspectus = AspectusMock::start().await;
    aspectus.mock_active_tenant("test-tenant").await;

    let registry = Arc::new(tenant::TenantRegistry::new());
    let runtime_config = Arc::new(harness_config);
    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(registry.clone(), runtime_config, None),
    );

    let config = api_gateway::ServerConfig::default();
    let aspectus_config = api_gateway::config::AspectusConfig {
        base_url: aspectus.base_url(),
        service_token: "test-service-token".into(),
        timeout_ms: 2000,
    };
    let state = Arc::new(
        api_gateway::AppState::new(manager, config, registry, &aspectus_config)
            .expect("build test app state"),
    );

    api_gateway::build_router(state)
}

/// Start a wiremock server that responds with an OpenAI SSE body.
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

// ── Response parsing helpers ──

/// Extract an HTTP response body as a UTF-8 string.
pub async fn text_body(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
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

/// Collect SSE events from an HTTP response body.
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

// ── Persistence helpers ──

use storage::session::postgres::PgSessionStore;

/// Verify Docker containers are running before persistence-dependent tests.
pub async fn ensure_test_containers() {
    let pg_url = std::env::var("PANDARIA_TEST_PG_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:15432/postgres".to_string());
    let redis_url = std::env::var("PANDARIA_TEST_REDIS_URL")
        .unwrap_or_else(|_| "redis://:redis@localhost:16379".to_string());

    let pg_ok = sqlx::PgPool::connect(&pg_url).await.is_ok();
    let redis_ok = redis::Client::open(redis_url.as_str()).map(|_| ()).is_ok();

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

/// Mock Aspectus server backed by wiremock.
pub struct AspectusMock {
    server: wiremock::MockServer,
}

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
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
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
                })),
            )
            .mount(&self.server)
            .await;
    }

    /// Mock a successful introspection with custom quota.
    pub async fn mock_tenant_with_quota(&self, tenant_id: &str, max_sessions: u32) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/introspect"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
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
                })),
            )
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

    /// Mock a tenant not configured for pandaria.
    pub async fn mock_no_pandaria_quota(&self, tenant_id: &str) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/introspect"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "active": true,
                    "tenant_id": tenant_id,
                    "user_id": "test-user",
                    "scope": "pandaria:session:create",
                })),
            )
            .mount(&self.server)
            .await;
    }

    /// Mock an internal server error from Aspectus.
    pub async fn mock_server_error(&self) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/introspect"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&self.server)
            .await;
    }

    /// Mock introspection that returns different tenants based on the token value.
    /// Takes a map of token → (tenant_id, max_sessions).
    /// Each request's `token` form field is matched against the map keys.
    pub async fn mock_tenants(&self, tenants: &[(&str, &str, u32)]) {
        use std::collections::HashMap;

        let responses: HashMap<String, serde_json::Value> = tenants
            .iter()
            .map(|(token, tenant_id, max_sessions)| {
                (
                    token.to_string(),
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
                )
            })
            .collect();

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/introspect"))
            .respond_with(move |req: &wiremock::Request| {
                // Parse the form body to extract the token
                let body = String::from_utf8_lossy(&req.body);
                for (token, response) in &responses {
                    if body.contains(&format!("token={}", token)) {
                        return wiremock::ResponseTemplate::new(200)
                            .set_body_json(response.clone());
                    }
                }
                // Unknown token → inactive
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"active": false}))
            })
            .mount(&self.server)
            .await;
    }
}

/// Build a test app with Aspectus auth enabled (internal driver).
pub async fn build_test_app_with_aspectus(
    provider: Arc<dyn ai_provider::LlmProvider>,
    aspectus: &AspectusMock,
) -> Router {
    build_test_app_with_aspectus_impl(provider, None, aspectus).await
}

/// Build a test app pointing at a raw Aspectus URL (for unavailable/scenario tests).
pub async fn build_test_app_with_aspectus_url(
    provider: Arc<dyn ai_provider::LlmProvider>,
    aspectus_url: String,
) -> Router {
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
        ssrf_policy: Arc::new(agent_core::utils::ssrf::SsrfPolicy::strict()),
        available_models: vec!["gpt-4".to_string()],
        compaction_config: agent_core::CompactionConfig::default(),
        agent_space: agent_core::AgentSpace::default(),
        hook_config: agent_core::HookConfig::default(),
        memory_store: None,
        session_retention_days: 7,
        session_cleanup_interval_hours: 24,
    });
    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(registry.clone(), runtime_config, None),
    );
    let config = api_gateway::ServerConfig::default();
    let aspectus_config = api_gateway::config::AspectusConfig {
        base_url: aspectus_url,
        service_token: "test-service-token".into(),
        timeout_ms: 2000,
    };
    let state = Arc::new(
        api_gateway::AppState::new(manager, config, registry, &aspectus_config)
            .expect("build test app state"),
    );
    api_gateway::build_router(state)
}

/// Build with a custom HTTP client.
pub async fn build_test_app_with_aspectus_and_client(
    provider: Arc<dyn ai_provider::LlmProvider>,
    client: reqwest::Client,
    aspectus: &AspectusMock,
) -> Router {
    build_test_app_with_aspectus_impl(provider, Some(client), aspectus).await
}

async fn build_test_app_with_aspectus_impl(
    provider: Arc<dyn ai_provider::LlmProvider>,
    http_client: Option<reqwest::Client>,
    aspectus: &AspectusMock,
) -> Router {
    let registry = Arc::new(tenant::TenantRegistry::new());
    let metrics_registry = Arc::new(observability::MetricsRegistry::new());

    let runtime_config = Arc::new(agent_core::HarnessConfig {
        provider,
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are a helpful assistant.".to_string(),
        default_context_window: 128_000,
        store: None,
        media_provider: None,
        media_registry: None,
        http_client: http_client.unwrap_or_else(reqwest::Client::new),
        ssrf_policy: Arc::new(agent_core::utils::ssrf::SsrfPolicy::strict()),
        available_models: vec!["gpt-4".to_string()],
        compaction_config: agent_core::CompactionConfig::default(),
        agent_space: agent_core::AgentSpace::default(),
        hook_config: agent_core::HookConfig::default(),
        memory_store: None,
        session_retention_days: 7,
        session_cleanup_interval_hours: 24,
    });
    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(
            registry.clone(),
            runtime_config,
            Some(metrics_registry.clone()),
        ),
    );

    let config = api_gateway::ServerConfig::default();
    let aspectus_config = api_gateway::config::AspectusConfig {
        base_url: aspectus.base_url(),
        service_token: "test-service-token".into(),
        timeout_ms: 2000,
    };
    let mut state = api_gateway::AppState::new(manager, config, registry, &aspectus_config)
        .expect("build test app state");
    state.metrics_registry = Some(metrics_registry);
    let state = Arc::new(state);

    api_gateway::build_router(state)
}
