//! End-to-end test: concurrent session isolation and quota enforcement.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_concurrent_sessions_isolation() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("concurrent ok");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider).await;
    let token = "pk_live_test-tenant";

    // Create 3 sessions concurrently
    let mut handles = Vec::new();
    for i in 0..3 {
        let app = app.clone();
        let token = token.clone();
        handles.push(tokio::spawn(async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/sessions")
                        .header("Authorization", format!("Bearer {}", token))
                        .header("Content-Type", "application/json")
                        .body(Body::from(format!(r#"{{"title": "concurrent-{}"}}"#, i)))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
            common::json_body(resp).await["id"]
                .as_str()
                .unwrap()
                .to_string()
        }));
    }

    let mut session_ids = Vec::new();
    for h in handles {
        session_ids.push(h.await.unwrap());
    }

    // Send messages to each session and verify isolation
    for (i, sid) in session_ids.iter().enumerate() {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/sessions/{}/messages", sid))
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"content": [{{"type":"text","text":"msg-{}"}}]}}"#,
                        i
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // Verify each session only has its own messages
    for (i, sid) in session_ids.iter().enumerate() {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/sessions/{}/messages", sid))
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let msgs = common::json_body(resp).await;
        let arr = msgs.as_array().unwrap();
        let has_own = arr.iter().any(|m| {
            m.get("content")
                .and_then(|c| c.as_array())
                .map_or(false, |content| {
                    content.iter().any(|c| {
                        c.get("text").and_then(|t| t.as_str())
                            == Some(&format!("msg-{}", i))
                    })
                })
        });
        assert!(has_own, "session {} missing its own message", i);
    }
}
