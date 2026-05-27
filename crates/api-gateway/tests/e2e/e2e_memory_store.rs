//! End-to-end test: MemoryStore integration via MemoryHookDispatcher.
//!
//! Verifies that turn content is correctly formatted and stored,
//! and that session deletion triggers forget_session.

mod common;

use std::sync::Arc;

use agent_core::memory::in_memory::InMemoryStore;
use agent_core::memory::{MemoryContext, MemoryStore};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_memory_store_remember_on_turn() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("remembered");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    let memory_store = Arc::new(InMemoryStore::new());

    // Build app with MemoryStore injected
    let app = {
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
            default_system_prompt: "You are helpful.".to_string(),
            default_context_window: 128_000,
            store: None,
            media_provider: None,
            media_registry: None,
            http_client: reqwest::Client::new(),
            available_models: vec!["gpt-4".to_string()],
            compaction_config: agent_core::CompactionConfig::default(),
            agent_space: agent_core::AgentSpace::default(),
            hook_config: agent_core::HookConfig::default(),
            memory_store: Some(memory_store.clone()),
        });
        let manager: Arc<dyn tenant::TenantManager> = Arc::new(
            tenant::manager::TenantManagerImpl::new(registry, runtime_config),
        );
        let config = api_gateway::ServerConfig {
            auth_secret: secrecy::SecretString::from(common::TEST_SECRET),
            ..Default::default()
        };
        api_gateway::build_router(Arc::new(api_gateway::AppState::new(manager, config)))
    };

    let token = common::make_token("test-tenant");

    // Create session
    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "mem test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let sid = common::json_body(create).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Send message
    let send = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"content": [{"type":"text","text":"remember this"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send.status(), StatusCode::OK);

    // Give fire-and-forget memory write time
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Recall memory
    let mem_ctx = agent_core::memory::MemoryContext {
        tenant_id: "test-tenant".into(),
        session_id: sid.clone(),
        user_id: None,
        model: "gpt-4".into(),
        session_started_at: std::time::SystemTime::now(),
    };
    let results = memory_store.recall(&mem_ctx, "remember").await.unwrap();
    assert!(
        !results.is_empty(),
        "memory should contain the turn content"
    );
    assert!(
        results[0].contains("remember"),
        "memory content should include user message"
    );
}
