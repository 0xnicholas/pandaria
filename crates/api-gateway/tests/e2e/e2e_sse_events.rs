//! End-to-end integration test: SSE event stream via HTTP API.
//!
//! Verifies that the full event pipeline (agent-core → tenant → api-gateway)
//! produces correctly formatted SSE events.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_sse_stream_receives_events() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("SSE works");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider).await;
    let token = "pk_live_test-tenant";

    // Create session
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "sse test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_body = common::json_body(create_response).await;
    let session_id = create_body["id"].as_str().unwrap();

    // Subscribe to SSE before sending the message.
    // tokio::sync::broadcast drops messages for receivers that haven't
    // subscribed yet, so we must ensure the SSE subscription is active
    // before the agent turn starts.
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

        assert_eq!(sse_response.status(), StatusCode::OK);
        let content_type = sse_response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(content_type.contains("text/event-stream"));

        // Signal that the broadcast subscription is now active.
        let _ = ready_tx.send(());
        common::collect_sse_events(sse_response).await
    });

    // Wait until the SSE subscription is confirmed before triggering the turn.
    ready_rx.await.expect("sse ready signal");

    // Send message (blocks until turn completes)
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

    // Keep app alive while SSE is being collected
    let _keep_alive = app;

    // Wait for SSE collection to finish (with timeout)
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), sse_handle)
        .await
        .unwrap()
        .unwrap();

    assert!(!events.is_empty(), "expected at least one SSE event");

    // Verify we have the expected event types
    let has_message_start = events
        .iter()
        .any(|e| matches!(e, api_gateway::types::ServerEvent::MessageStart { .. }));
    let has_turn_end = events
        .iter()
        .any(|e| matches!(e, api_gateway::types::ServerEvent::TurnEnd { .. }));

    assert!(has_message_start, "expected MessageStart event");
    assert!(has_turn_end, "expected TurnEnd event");

    // Verify TurnEnd has correct stop_reason
    let turn_end = events.iter().find_map(|e| match e {
        api_gateway::types::ServerEvent::TurnEnd { stop_reason, .. } => Some(stop_reason.clone()),
        _ => None,
    });
    assert_eq!(turn_end, Some("stop".to_string()));
}

#[tokio::test]
async fn test_sse_text_delta_content() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("delta content");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider).await;
    let token = "pk_live_test-tenant";

    // Create session
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "sse delta test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let create_body = common::json_body(create_response).await;
    let session_id = create_body["id"].as_str().unwrap();

    // Subscribe to SSE before sending the message.
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

    // Send message
    let _send_response = app
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

    // Keep app alive while SSE is being collected
    let _keep_alive = app;

    // Collect events
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), sse_handle)
        .await
        .unwrap()
        .unwrap();

    // Concatenate all text deltas
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            api_gateway::types::ServerEvent::TextDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(text, "delta content");
}
