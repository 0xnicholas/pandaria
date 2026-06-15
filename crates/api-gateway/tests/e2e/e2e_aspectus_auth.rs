#[cfg(feature = "aspectus-auth")]
mod common;

#[cfg(feature = "aspectus-auth")]
use common::*;

#[cfg(feature = "aspectus-auth")]
use axum::body::Body;
#[cfg(feature = "aspectus-auth")]
use axum::http::Request;
#[cfg(feature = "aspectus-auth")]
use axum::http::StatusCode;
#[cfg(feature = "aspectus-auth")]
use tower::ServiceExt;

#[cfg(feature = "aspectus-auth")]
#[tokio::test]
async fn test_create_session_with_aspectus_auth() {
    let aspectus = AspectusMock::start().await;
    aspectus.mock_active_tenant("test-tenant").await;

    let provider: std::sync::Arc<dyn ai_provider::LlmProvider> = std::sync::Arc::new(
        ai_provider::providers::openai::OpenAiProvider::with_base_url(
            Some(secrecy::SecretString::from("sk-test")),
            "http://localhost:1",
        ),
    );

    let router = build_test_app_with_aspectus(provider, aspectus.base_url()).await;

    let resp = router
        .oneshot(
            Request::post("/api/v1/sessions")
                .header("Authorization", "Bearer pk_live_test123")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title":"aspectus-test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "expected 201, got {}",
        resp.status()
    );
}

#[cfg(feature = "aspectus-auth")]
#[tokio::test]
async fn test_inactive_token_rejected() {
    let aspectus = AspectusMock::start().await;
    aspectus.mock_inactive().await;

    let provider: std::sync::Arc<dyn ai_provider::LlmProvider> = std::sync::Arc::new(
        ai_provider::providers::openai::OpenAiProvider::with_base_url(
            Some(secrecy::SecretString::from("sk-test")),
            "http://localhost:1",
        ),
    );

    let router = build_test_app_with_aspectus(provider, aspectus.base_url()).await;

    let resp = router
        .oneshot(
            Request::post("/api/v1/sessions")
                .header("Authorization", "Bearer expired_token")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title":"test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[cfg(feature = "aspectus-auth")]
#[tokio::test]
async fn test_no_pandaria_quota_rejected() {
    let aspectus = AspectusMock::start().await;
    aspectus.mock_no_pandaria_quota("test-tenant").await;

    let provider: std::sync::Arc<dyn ai_provider::LlmProvider> = std::sync::Arc::new(
        ai_provider::providers::openai::OpenAiProvider::with_base_url(
            Some(secrecy::SecretString::from("sk-test")),
            "http://localhost:1",
        ),
    );

    let router = build_test_app_with_aspectus(provider, aspectus.base_url()).await;

    let resp = router
        .oneshot(
            Request::post("/api/v1/sessions")
                .header("Authorization", "Bearer pk_live_test123")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title":"test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
