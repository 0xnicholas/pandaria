//! End-to-end test: agent loop completes despite persistence failures.
//!
//! Uses a FailingStore that always returns errors on write to verify
//! that persistence failure never blocks the agent loop.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// A SessionStore that always returns errors on write.
struct FailingStore;

#[async_trait::async_trait]
impl agent_core::SessionStore for FailingStore {
    async fn save_session(
        &self,
        _: &str,
        _: &str,
        _: &[agent_core::SessionEntry],
    ) -> Result<(), agent_core::AgentError> {
        Err(agent_core::AgentError::Persistence(
            "simulated failure".into(),
        ))
    }

    async fn load_session(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Vec<agent_core::SessionEntry>, agent_core::AgentError> {
        Ok(Vec::new())
    }

    async fn delete_session(
        &self,
        _: &str,
        _: &str,
    ) -> Result<(), agent_core::AgentError> {
        Ok(())
    }

    async fn list_sessions(
        &self,
        _: &str,
    ) -> Result<Vec<String>, agent_core::AgentError> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn test_agent_loop_survives_persistence_failure() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("success despite failure");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    let failing_store: Arc<dyn agent_core::SessionStore> = Arc::new(FailingStore);
    let app = common::build_test_app_with_store(provider, failing_store);
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
                .body(Body::from(r#"{"title": "fault test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_body = common::json_body(create_response).await;
    let session_id = create_body["id"].as_str().unwrap();

    // Send message — should complete normally despite persistence failure
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
    let send_body = common::json_body(send_response).await;
    assert_eq!(send_body["turn_index"], 0);

    // Verify we still get back messages (in-memory state is intact)
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

    assert_eq!(msgs_response.status(), StatusCode::OK);
    let msgs = common::json_body(msgs_response).await;
    let msgs_arr = msgs.as_array().unwrap();
    assert!(
        msgs_arr.len() >= 2,
        "expected messages in memory despite persistence failure"
    );
}
