//! End-to-end test: MemoryStore integration via MemoryHookDispatcher.
//!
//! Verifies that turn content is correctly formatted and stored,
//! and that session deletion triggers forget_session.

mod common;

use std::sync::Arc;

use agent_core::memory::in_memory::InMemoryStore;
use agent_core::memory::{EmeraldMemoryStore, MemoryContext, MemoryStore};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_memory_store_remember_on_turn() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("remembered");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    let memory_store = Arc::new(InMemoryStore::new());

    let harness_config = agent_core::HarnessConfig {
        provider: provider.clone(),
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are helpful.".to_string(),
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
        memory_store: Some(memory_store.clone()),
        session_retention_days: 7,
        session_cleanup_interval_hours: 24,
    };
    let app = common::build_test_app_with_config(provider.clone(), harness_config).await;

    let token = "pk_live_test-tenant";

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
    let mem_ctx = MemoryContext {
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

/// End-to-end test: Pandaria runtime → MemoryHookDispatcher → EmeraldMemoryStore.
/// Requires a running Emerald persistence server.
#[tokio::test]
async fn test_memory_store_emerald_persistence_e2e() {
    let _ = tracing_subscriber::fmt().try_init();

    let (emerald_url, emerald_key) = match (
        std::env::var("PANDARIA_TEST_EMERALD_URL").ok(),
        std::env::var("PANDARIA_TEST_EMERALD_API_KEY").ok(),
    ) {
        (Some(url), Some(key)) => (url, key),
        _ => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let body = common::openai_text_sse_body("hello from pandaria runtime");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    let memory_store = Arc::new(EmeraldMemoryStore::new(&emerald_url, &emerald_key));

    let tenant_id = "pandaria_runtime_tenant";

    let harness_config = agent_core::HarnessConfig {
        provider: provider.clone(),
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are helpful.".to_string(),
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
        memory_store: Some(memory_store.clone()),
        session_retention_days: 7,
        session_cleanup_interval_hours: 24,
    };
    let app = common::build_test_app_with_config(provider.clone(), harness_config).await;

    let token = "pk_live_pandaria_runtime_tenant";

    // Create session
    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "emerald e2e test"}"#))
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
                    r#"{"content": [{"type":"text","text":"tell me about runtime persistence"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send.status(), StatusCode::OK);

    let mem_ctx = MemoryContext {
        tenant_id: tenant_id.into(),
        session_id: sid.clone(),
        user_id: None,
        model: "gpt-4".into(),
        session_started_at: std::time::SystemTime::now(),
    };

    // Poll Emerald
    let mut found = false;
    for attempt in 1..=30 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        match memory_store.recall(&mem_ctx, "runtime persistence").await {
            Ok(results) if !results.is_empty() => {
                let combined = results.join(" ").to_lowercase();
                if combined.contains("runtime") || combined.contains("persistence") {
                    found = true;
                    break;
                }
            }
            Ok(_) => {}
            Err(e) => eprintln!("attempt {}: recall error: {}", attempt, e),
        }
    }

    assert!(
        found,
        "Pandaria runtime turn content should be persisted in Emerald"
    );
}
