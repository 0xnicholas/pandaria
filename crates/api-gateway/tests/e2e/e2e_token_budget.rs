//! End-to-end integration test: TokenBudget non-blocking turn counting.
//!
//! Verifies that `DefaultHookDispatcher` counts turns per session and logs
//! warnings when `max_turns_per_session` is exceeded, but does NOT block
//! the agent loop.

mod common;

use std::sync::Arc;

use agent_core::harness::config::HookConfig;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

async fn build_app_with_token_budget(
    provider: Arc<dyn ai_provider::LlmProvider>,
    max_turns: usize,
) -> axum::Router {
    let mut hook_config = HookConfig::default();
    hook_config.max_turns_per_session = max_turns;

    let harness_config = agent_core::HarnessConfig {
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
        hook_config,
        memory_store: None,
        session_retention_days: 7,
        session_cleanup_interval_hours: 24,
    };
    common::build_test_app_with_config(provider, harness_config).await
}

#[tokio::test]
async fn test_token_budget_does_not_block() {
    let _ = tracing_subscriber::fmt().try_init();

    // LLM always returns simple text (no tool calls)
    let body = common::openai_text_sse_body("ok");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = build_app_with_token_budget(provider, 1).await;
    let token = "pk_live_test-tenant";

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "budget"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let sid = common::json_body(create).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Turn 1: within budget
    let r1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"content": [{"type":"text","text":"first"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);

    // Turn 2: exceeds max_turns=1, but should NOT block
    let r2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"content": [{"type":"text","text":"second"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::OK);

    // Turn 3: still should NOT block
    let r3 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"content": [{"type":"text","text":"third"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r3.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_token_budget_turn_count_tracked() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("ok");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = build_app_with_token_budget(provider, 5).await;
    let token = "pk_live_test-tenant";

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "budget-count"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = common::json_body(create).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Send 3 messages
    for i in 0..3 {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/sessions/{}/messages", sid))
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"content": [{{"type":"text","text":"msg {}"}}]}}"#,
                        i
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }

    // Verify session turn count via GET /sessions/{id}
    let get = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}", sid))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let info = common::json_body(get).await;
    let turn_count = info["turn_count"].as_u64().unwrap_or(0);
    assert_eq!(
        turn_count, 3,
        "session turn count should reflect 3 messages"
    );
}
