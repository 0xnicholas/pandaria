//! End-to-end integration test: persistence recovery across simulated restarts.
//!
//! Verifies session history survives service restart via PostgreSQL store.

mod common;

use std::sync::Arc;

use agent_core::SessionStore;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use storage::session::postgres::PgSessionStore;
use tower::ServiceExt;

#[tokio::test]
async fn test_session_persistence_recovery() {
    let _ = tracing_subscriber::fmt().try_init();
    common::ensure_test_containers().await;

    let body = common::openai_text_sse_body("persisted response");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    // Phase 1: Create session, send message, flush, then "restart"
    let store = Arc::new(common::create_test_pg_store().await);
    let session_id;
    {
        let app = common::build_test_app_with_store(provider.clone(), store.clone()).await;
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
                    .body(Body::from(r#"{"title": "recovery test"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let create_body = common::json_body(create_response).await;
        session_id = create_body["id"].as_str().unwrap().to_string();

        // Send message
        let send_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/sessions/{}/messages", session_id))
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        r#"{"content": [{"type":"text","text":"persist me"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(send_response.status(), StatusCode::OK);

        // Give fire-and-forget save time to complete
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    // App dropped here — simulates app restart (store persists independently)

    // Phase 2: Verify persisted data in store directly
    {
        let entries = store
            .load_session("test-tenant", &session_id)
            .await
            .expect("load session from pg");

        assert!(
            entries.len() >= 2,
            "expected at least user + assistant in store, got {}",
            entries.len()
        );

        // Verify user message survived
        let has_user = entries.iter().any(|e| {
            if let agent_core::SessionEntry::Message { message, .. } = e {
                if let agent_core::AgentMessage::User(u) = message {
                    return u.content.iter().any(|c| {
                        if let ai_provider::Content::Text { text, .. } = c {
                            text == "persist me"
                        } else {
                            false
                        }
                    });
                }
            }
            false
        });
        assert!(has_user, "user message not found in store after recovery");

        // Verify assistant response survived
        let has_assistant = entries.iter().any(|e| {
            if let agent_core::SessionEntry::Message { message, .. } = e {
                if let agent_core::AgentMessage::Assistant(a) = message {
                    return a.content.iter().any(|c| {
                        if let ai_provider::Content::Text { text, .. } = c {
                            text == "persisted response"
                        } else {
                            false
                        }
                    });
                }
            }
            false
        });
        assert!(
            has_assistant,
            "assistant message not found in store after recovery"
        );
    }
}
