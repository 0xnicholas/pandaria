//! End-to-end integration test: full session lifecycle via HTTP API.
//!
//! Uses real `TenantManagerImpl` + wiremock OpenAI provider.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_session_lifecycle() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("Hello from E2E");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider).await;
    let token = "pk_live_test-tenant";

    // 1. Create session
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "e2e test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_body = common::json_body(create_response).await;
    let session_id = create_body["id"].as_str().unwrap();
    assert_eq!(create_body["title"], "e2e test");

    // 2. Send message (blocks until agent turn completes)
    let send_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"content": [{"type":"text","text":"hi"}]}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(send_response.status(), StatusCode::OK);
    let send_body = common::json_body(send_response).await;
    assert_eq!(send_body["turn_index"], 0);

    // 3. Get session messages — should contain user + assistant
    let msgs_response = app
        .clone()
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

    assert_eq!(msgs_response.status(), StatusCode::OK);
    let msgs = common::json_body(msgs_response).await;
    let msgs_arr = msgs.as_array().unwrap();
    assert_eq!(msgs_arr.len(), 2, "expected user + assistant messages");

    // Verify assistant message contains the text
    let assistant = &msgs_arr[1];
    let content = assistant["content"].as_array().unwrap();
    let text_parts: Vec<String> = content
        .iter()
        .filter_map(|c| c["text"].as_str().map(|s| s.to_string()))
        .collect();
    let full_text = text_parts.join("");
    assert_eq!(full_text, "Hello from E2E");

    // 4. Update session
    let update_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/sessions/{}", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "updated", "model": "gpt-5"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(update_response.status(), StatusCode::OK);
    let update_body = common::json_body(update_response).await;
    assert_eq!(update_body["title"], "updated");
    assert_eq!(update_body["model"], "gpt-5");

    // 5. List sessions
    let list_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(list_response.status(), StatusCode::OK);
    let list_body = common::json_body(list_response).await;
    let sessions = list_body.as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], session_id);

    // 6. Delete session
    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/sessions/{}", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    // 7. Verify session is gone
    let get_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
}
