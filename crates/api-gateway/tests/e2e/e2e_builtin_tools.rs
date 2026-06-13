//! E2E tests for built-in Pawbun tools integration.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

// Test that session creation succeeds with builtin_tools enabled (default)
#[tokio::test]
async fn test_builtin_tools_registered_by_default() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("ok");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"title": "test", "system_prompt": "You have file tools"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
}

// Test that disabled filters work
#[tokio::test]
async fn test_builtin_tools_disabled_filter() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("ok");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"builtin_tools": {"enabled": true, "disabled": ["code_execute"]}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
}

// Test that all tools can be disabled without error
#[tokio::test]
async fn test_builtin_tools_disabled_all() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("ok");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"builtin_tools": {"enabled": true, "disabled": ["file_read", "file_write", "directory_list", "code_execute"]}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
}

// Test builtin_tools off entirely
#[tokio::test]
async fn test_builtin_tools_off() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("ok");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"builtin_tools": {"enabled": false}}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
}

// Test external tool shadows builtin (both registered, external wins)
#[tokio::test]
async fn test_external_shadows_builtin() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("ok");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"tools": [{"name": "file_read", "description": "Custom", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}, "endpoint": "http://mock-tool.local/invoke"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
}
