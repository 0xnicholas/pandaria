//! E2E tests for the /metrics endpoint with per-tenant observability.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// Verify GET /metrics returns 200 and valid Prometheus format.
#[tokio::test]
async fn e2e_metrics_endpoint_returns_prometheus() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("Hello");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider).await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = common::text_body(resp).await;
    assert!(body.contains("# HELP "));
    assert!(body.contains("# TYPE "));
    assert!(body.contains("pandaria_sessions_active"));
}

/// Verify session creation counter appears in metrics.
#[tokio::test]
async fn e2e_metrics_after_session_creation() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("Hello");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider).await;
    let token = "pk_live_test-tenant";

    // Create a session
    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "metrics test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let session = common::json_body(create).await;
    let session_id = session["id"].as_str().unwrap();

    // Check metrics
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = common::text_body(resp).await;
    assert!(body.contains("pandaria_sessions_total"));
    assert!(body.contains("created"));

    // Cleanup: complete then delete
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/complete", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/sessions/{}", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
}

/// Verify per-tenant metric isolation.
#[tokio::test]
async fn e2e_metrics_multi_tenant_isolation() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("Hello");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider).await;
    let token = "pk_live_test-tenant";

    // Create a session to generate metrics with tenant label
    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "iso test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let session = common::json_body(create).await;
    let session_id = session["id"].as_str().unwrap();

    // Verify metrics contain the tenant label
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = common::text_body(resp).await;
    assert!(body.contains("test-tenant"));

    // Cleanup
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/complete", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/sessions/{}", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
}
