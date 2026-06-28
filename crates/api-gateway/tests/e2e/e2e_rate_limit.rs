//! End-to-end integration test: rate limiting via TokenBucket.
//!
//! Verifies that the `rate_limit_middleware` returns HTTP 429 after the
//! configured burst size is exceeded, and that buckets are per-tenant.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// Build a test app with custom rate limit config and mock Aspectus.
async fn build_app_with_rate_limit(
    provider: Arc<dyn ai_provider::LlmProvider>,
    aspectus: &common::AspectusMock,
) -> axum::Router {
    let registry = Arc::new(tenant::TenantRegistry::new());

    let runtime_config = Arc::new(agent_core::HarnessConfig {
        provider: provider.clone(),
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

    let config = api_gateway::ServerConfig {
        rate_limit: api_gateway::RateLimitConfig {
            requests_per_second: 100,
            burst_size: 1,
        },
        ..Default::default()
    };
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

#[tokio::test]
async fn test_rate_limit_per_tenant_isolation() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("ok");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    let aspectus = common::AspectusMock::start().await;
    aspectus.mock_active_tenant("tenant-a").await;
    aspectus.mock_active_tenant("tenant-b").await;
    let app = build_app_with_rate_limit(provider, &aspectus).await;

    let token_a = "pk_live_tenant-a";
    let token_b = "pk_live_tenant-b";

    // Tenant A exhausts its burst (burst_size=1)
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token_a))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "a1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Second request from A should be rate limited
    let r_a2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token_a))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "a2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r_a2.status(), StatusCode::TOO_MANY_REQUESTS);

    // Tenant B should still be able to create (separate bucket)
    let r_b1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token_b))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "b1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r_b1.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_rate_limit_refills() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("ok");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    let aspectus = common::AspectusMock::start().await;
    aspectus.mock_active_tenant("test-tenant").await;
    let app = build_app_with_rate_limit(provider, &aspectus).await;

    let token = "pk_live_test-tenant";

    // Exhaust burst
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "first"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let r2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "second"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::TOO_MANY_REQUESTS);

    // Wait for refill
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let r3 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "after-refill"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r3.status(), StatusCode::CREATED);
}
