//! End-to-end integration test: rate limiting via TokenBucket.
//!
//! Verifies that the `rate_limit_middleware` returns HTTP 429 after the
//! configured burst size is exhausted, and that buckets are per-tenant.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

fn build_app_with_rate_limit(
    provider: Arc<dyn ai_provider::LlmProvider>,
    rps: u32,
    burst: u32,
) -> axum::Router {
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

    let config = api_gateway::ServerConfig {
        auth_secret: secrecy::SecretString::from(common::TEST_SECRET),
        rate_limit: api_gateway::RateLimitConfig {
            requests_per_second: rps,
            burst_size: burst,
        },
        ..Default::default()
    };
    let state = Arc::new(api_gateway::AppState::new(manager, config));
    api_gateway::build_router(state)
}

#[tokio::test]
async fn test_rate_limit_blocks_after_burst() {
    let _ = tracing_subscriber::fmt().try_init();

    let (_server, provider) = common::start_wiremock_openai(
        &common::openai_text_sse_body("ok")
    ).await;
    let app = build_app_with_rate_limit(provider, 1, 2);
    let token = common::make_token("test-tenant");

    // Request 1: within burst → 201
    let r1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "r1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::CREATED);

    // Request 2: within burst → 201
    let r2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "r2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::CREATED);

    // Request 3: burst exhausted → 429
    let r3 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "r3"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r3.status(), StatusCode::TOO_MANY_REQUESTS);

    // Verify Retry-After header is present
    let retry_after = r3
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("0");
    let retry_secs: u64 = retry_after.parse().unwrap_or(0);
    assert!(retry_secs > 0, "expected Retry-After header > 0");
}

#[tokio::test]
async fn test_rate_limit_per_tenant_isolation() {
    let _ = tracing_subscriber::fmt().try_init();

    let (_server, provider) = common::start_wiremock_openai(
        &common::openai_text_sse_body("ok")
    ).await;

    // Register two tenants with the same rate limit config
    let registry = Arc::new(tenant::TenantRegistry::new());
    let t1 = tenant::Tenant::new(
        "tenant-a",
        tenant::TenantQuota {
            max_concurrent_sessions: 10,
            max_tokens_per_day: 1_000_000,
            max_tool_calls_per_minute: 60,
            cpu_time_budget_ms_per_day: 3_600_000,
        },
    );
    let t2 = tenant::Tenant::new(
        "tenant-b",
        tenant::TenantQuota {
            max_concurrent_sessions: 10,
            max_tokens_per_day: 1_000_000,
            max_tool_calls_per_minute: 60,
            cpu_time_budget_ms_per_day: 3_600_000,
        },
    );
    registry.register(t1).unwrap();
    registry.register(t2).unwrap();

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

    let config = api_gateway::ServerConfig {
        auth_secret: secrecy::SecretString::from(common::TEST_SECRET),
        rate_limit: api_gateway::RateLimitConfig {
            requests_per_second: 100,
            burst_size: 1,
        },
        ..Default::default()
    };
    let state = Arc::new(api_gateway::AppState::new(manager, config));
    let app = api_gateway::build_router(state);

    let token_a = common::make_token("tenant-a");
    let token_b = common::make_token("tenant-b");

    // Tenant A exhausts its burst
    let r_a1 = app
        .clone()
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
    assert_eq!(r_a1.status(), StatusCode::CREATED);

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

    // Tenant B should still be allowed
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
async fn test_rate_limit_refills_after_delay() {
    let _ = tracing_subscriber::fmt().try_init();

    let (_server, provider) = common::start_wiremock_openai(
        &common::openai_text_sse_body("ok")
    ).await;
    let app = build_app_with_rate_limit(provider, 100, 1);
    let token = common::make_token("test-tenant");

    // Exhaust burst
    let r1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "r1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::CREATED);

    let r2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "r2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::TOO_MANY_REQUESTS);

    // Wait for refill (100 rps → 10ms per token, wait 50ms to be safe)
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let r3 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "r3"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r3.status(), StatusCode::CREATED);
}
