//! End-to-end integration test: tenant isolation and quota enforcement.
//!
//! Verifies that:
//! - Tenants cannot access each other's sessions
//! - SSE events are isolated per tenant
//! - Session quota limits are enforced

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_tenant_session_isolation() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("isolated");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    // Register two tenants
    let registry = Arc::new(tenant::TenantRegistry::new());
    let t1 = tenant::Tenant::new("tenant-1", tenant::TenantQuota::default());
    let t2 = tenant::Tenant::new("tenant-2", tenant::TenantQuota::default());
    registry.register(t1).unwrap();
    registry.register(t2).unwrap();

    let app = common::build_test_app_with_registry(provider, registry);
    let token_t1 = "pk_live_tenant-1";
    let token_t2 = "pk_live_tenant-2";

    // Tenant 1 creates a session
    let create_t1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token_t1))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "t1 session"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_t1.status(), StatusCode::CREATED);
    let body_t1 = common::json_body(create_t1).await;
    let session_id_t1 = body_t1["id"].as_str().unwrap();

    // Tenant 2 tries to access tenant-1's session → 404
    let get_t2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}", session_id_t1))
                .header("Authorization", format!("Bearer {}", token_t2))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(get_t2.status(), StatusCode::NOT_FOUND);

    // Tenant 2 tries to send message to tenant-1's session → 404
    let send_t2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", session_id_t1))
                .header("Authorization", format!("Bearer {}", token_t2))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"content": [{"type":"text","text":"hi"}]}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(send_t2.status(), StatusCode::NOT_FOUND);

    // Tenant 2 tries to subscribe to tenant-1's SSE → 404
    let sse_t2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}/events", session_id_t1))
                .header("Authorization", format!("Bearer {}", token_t2))
                .header("Accept", "text/event-stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(sse_t2.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_session_quota_limit_enforced() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("quota");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    // Register tenant with limit of 1 concurrent session
    let registry = Arc::new(tenant::TenantRegistry::new());
    let t1 = tenant::Tenant::new(
        "quota-tenant",
        tenant::TenantQuota {
            max_concurrent_sessions: 1,
            ..tenant::TenantQuota::default()
        },
    );
    registry.register(t1).unwrap();

    let app = common::build_test_app_with_registry(provider, registry);
    let token = "pk_live_quota-tenant";

    // First session succeeds
    let create1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "first"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create1.status(), StatusCode::CREATED);

    // Second session should fail with 429 Too Many Requests (mapped from SessionLimitExceeded)
    let create2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "second"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create2.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_delete_releases_session_slot() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("release");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    let registry = Arc::new(tenant::TenantRegistry::new());
    let t1 = tenant::Tenant::new(
        "release-tenant",
        tenant::TenantQuota {
            max_concurrent_sessions: 1,
            ..tenant::TenantQuota::default()
        },
    );
    registry.register(t1).unwrap();

    let app = common::build_test_app_with_registry(provider, registry);
    let token = "pk_live_release-tenant";

    // Create first session
    let create1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "first"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create1.status(), StatusCode::CREATED);
    let body1 = common::json_body(create1).await;
    let session_id = body1["id"].as_str().unwrap();

    // Delete it
    let delete = app
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

    assert_eq!(delete.status(), StatusCode::NO_CONTENT);

    // Now we should be able to create another session
    let create2 = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "second"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create2.status(), StatusCode::CREATED);
}
