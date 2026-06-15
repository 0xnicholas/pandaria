//! End-to-end integration test: tenant isolation and quota enforcement.

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

    let aspectus = common::AspectusMock::start().await;
    aspectus
        .mock_tenants(&[
            ("pk_live_tenant-1", "tenant-1", 10),
            ("pk_live_tenant-2", "tenant-2", 10),
        ])
        .await;
    let app = common::build_test_app_with_aspectus(provider, &aspectus).await;

    let token_t1 = "pk_live_tenant-1";
    let token_t2 = "pk_live_tenant-2";

    // Tenant 1 creates a session
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token_t1))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "t1-session"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let sid_t1 = common::json_body(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Tenant 2 creates a session
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token_t2))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "t2-session"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let sid_t2 = common::json_body(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Tenant 2 cannot access Tenant 1's session
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}", sid_t1))
                .header("Authorization", format!("Bearer {}", token_t2))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Tenant 1 can access its own session
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}", sid_t1))
                .header("Authorization", format!("Bearer {}", token_t1))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_session_quota_limit_enforced() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("quota");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    let aspectus = common::AspectusMock::start().await;
    aspectus
        .mock_tenants(&[("pk_live_quota-tenant", "quota-tenant", 2)])
        .await;
    let app = common::build_test_app_with_aspectus(provider, &aspectus).await;

    let token = "pk_live_quota-tenant";

    for i in 1..=2 {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions")
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(format!(r#"{{"title": "session-{}"}}"#, i)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "session {i} should create"
        );
    }

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "session-3"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_session_released_on_delete() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("release");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    let aspectus = common::AspectusMock::start().await;
    aspectus
        .mock_tenants(&[("pk_live_release-tenant", "release-tenant", 1)])
        .await;
    let app = common::build_test_app_with_aspectus(provider, &aspectus).await;

    let token = "pk_live_release-tenant";

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "only-slot"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let sid = common::json_body(create).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/sessions/{}", sid))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "new-slot"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}
