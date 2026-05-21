//! End-to-end integration test: API extensions (quota, batch, clone, reset, sync wait).

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_get_quota() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("quota");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/tenant/quota")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let quota = common::json_body(response).await;

    assert_eq!(quota["tenant_id"], "test-tenant");
    assert!(quota["max_concurrent_sessions"].is_u64());
    assert!(quota["active_sessions"].is_u64());
    assert!(quota["max_tokens_per_day"].is_u64());
    assert!(quota["tokens_used_today"].is_u64());
    assert!(quota["max_tool_calls_per_minute"].is_u64());
    assert!(quota["tool_calls_in_last_minute"].is_u64());
    assert!(quota["default_model"].is_string());
    assert!(quota["available_models"].is_array());
}

#[tokio::test]
async fn test_batch_create_sessions() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("batch");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions/batch")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"count": 3, "template": {"title": "batch test"}}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let result = common::json_body(response).await;
    let created = result["created"].as_array().unwrap();
    assert_eq!(created.len(), 3);

    // Verify each session has a unique id
    let ids: std::collections::HashSet<String> = created
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 3, "expected 3 unique session ids");

    let failed = result["failed"].as_array().unwrap();
    assert!(failed.is_empty());
}

#[tokio::test]
async fn test_clone_session() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("clone");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    // Create original session
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "original", "system_prompt": "you are a tester"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let create_body = common::json_body(create_response).await;
    let original_id = create_body["id"].as_str().unwrap();

    // Clone it
    let clone_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/clone", original_id))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "cloned"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(clone_response.status(), StatusCode::CREATED);
    let clone_body = common::json_body(clone_response).await;
    let cloned_id = clone_body["id"].as_str().unwrap();

    assert_ne!(cloned_id, original_id, "cloned session must have a new id");
    assert_eq!(clone_body["title"], "cloned");

    // Verify cloned session has empty message history
    let msgs_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}/messages", cloned_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let msgs = common::json_body(msgs_response).await;
    let msgs_arr = msgs.as_array().unwrap();
    assert_eq!(msgs_arr.len(), 0, "cloned session should have empty history");
}

#[tokio::test]
async fn test_reset_session() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("reset");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    // Create session
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "reset test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let create_body = common::json_body(create_response).await;
    let session_id = create_body["id"].as_str().unwrap();

    // Send a message so history is non-empty
    let _send_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"content": [{"type":"text","text":"hello"}]}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Reset
    let reset_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/reset", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(reset_response.status(), StatusCode::OK);
    let reset_body = common::json_body(reset_response).await;
    assert_eq!(reset_body["state"], "idle");

    // Verify history is cleared
    let msgs_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}/messages", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let msgs = common::json_body(msgs_response).await;
    let msgs_arr = msgs.as_array().unwrap();
    assert_eq!(msgs_arr.len(), 0, "history should be empty after reset");
}

#[tokio::test]
async fn test_sync_wait_success() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("sync wait");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    // Create session
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "wait test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let create_body = common::json_body(create_response).await;
    let session_id = create_body["id"].as_str().unwrap();

    // Send message with wait=true
    let wait_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/sessions/{}/messages?wait=true&timeout_ms=10000",
                    session_id
                ))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"content": [{"type":"text","text":"hello"}]}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Turn completes quickly with wiremock, so expect 200
    assert_eq!(wait_response.status(), StatusCode::OK);
    let wait_body = common::json_body(wait_response).await;
    assert_eq!(wait_body["completed"], true);
    assert!(wait_body["turn_index"].is_u64());
    assert!(wait_body.get("messages").is_some());
}
