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
async fn test_session_limit_enforced_from_aspectus_quota() {
    let aspectus = AspectusMock::start().await;
    // Only allow 1 concurrent session
    aspectus.mock_tenant_with_quota("test-tenant", 1).await;

    let provider: std::sync::Arc<dyn ai_provider::LlmProvider> = std::sync::Arc::new(
        ai_provider::providers::openai::OpenAiProvider::with_base_url(
            Some(secrecy::SecretString::from("sk-test")),
            "http://localhost:1",
        ),
    );

    let router = build_test_app_with_aspectus(provider, aspectus.base_url()).await;

    // First session: should succeed
    let resp1 = router
        .clone()
        .oneshot(
            Request::post("/api/v1/sessions")
                .header("Authorization", "Bearer pk_live_test123")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title":"session-1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp1.status(),
        StatusCode::CREATED,
        "first session should be created"
    );

    // Second session: should be rejected (limit = 1)
    let resp2 = router
        .oneshot(
            Request::post("/api/v1/sessions")
                .header("Authorization", "Bearer pk_live_test123")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title":"session-2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        resp2.status().is_client_error(),
        "second session should be rejected, got {}",
        resp2.status()
    );
}
