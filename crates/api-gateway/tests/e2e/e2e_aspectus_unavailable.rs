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
async fn test_aspectus_unavailable_returns_503() {
    // Don't start AspectusMock — use an unreachable URL
    let provider: std::sync::Arc<dyn ai_provider::LlmProvider> = std::sync::Arc::new(
        ai_provider::providers::openai::OpenAiProvider::with_base_url(
            Some(secrecy::SecretString::from("sk-test")),
            "http://localhost:1",
        ),
    );

    // Use a port that likely has nothing listening
    let router = build_test_app_with_aspectus(provider, "http://127.0.0.1:19999".to_string()).await;

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

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[cfg(feature = "aspectus-auth")]
#[tokio::test]
async fn test_aspectus_server_error_returns_503() {
    let aspectus = AspectusMock::start().await;
    aspectus.mock_server_error().await;

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

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}
