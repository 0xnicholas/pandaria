//! End-to-end integration test: session state machine exposure.
//!
//! Verifies GET /sessions/{id}/state returns correct state values.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_session_state_lifecycle() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("state test");
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
                .body(Body::from(r#"{"title": "state test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_body = common::json_body(create_response).await;
    let session_id = create_body["id"].as_str().unwrap();

    // 1. Before any turn: state should be idle
    let state_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}/state", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(state_response.status(), StatusCode::OK);
    let state_body = common::json_body(state_response).await;
    assert_eq!(state_body["state"], "idle");
    assert!(state_body.get("error_reason").is_some());

    // Subscribe SSE before sending message
    let sse_app = app.clone();
    let sse_token = token.clone();
    let sse_session_id = session_id.to_string();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

    let sse_handle = tokio::spawn(async move {
        let sse_response = sse_app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/sessions/{}/events", sse_session_id))
                    .header("Authorization", format!("Bearer {}", sse_token))
                    .header("Accept", "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let _ = ready_tx.send(());
        common::collect_sse_events(sse_response).await
    });

    ready_rx.await.expect("sse ready signal");

    // 2. Send message to trigger a turn
    let send_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"content": [{"type":"text","text":"hello"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(send_response.status(), StatusCode::OK);

    // 3. After turn completes: state should return to idle
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), sse_handle)
        .await
        .unwrap()
        .unwrap();

    assert!(!events.is_empty());

    let state_after_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}/state", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let state_after = common::json_body(state_after_response).await;
    assert_eq!(state_after["state"], "idle");
    assert!(state_after.get("error_reason").is_some());
}

#[tokio::test]
async fn test_session_state_not_found() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("not found");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    let state_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/sessions/00000000-0000-0000-0000-000000000000/state")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(state_response.status(), StatusCode::NOT_FOUND);
}
