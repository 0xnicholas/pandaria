//! End-to-end integration test: webhook event delivery.
//!
//! Verifies that session events are delivered to a configured webhook endpoint
//! with correct headers, body, and HMAC-SHA256 signature.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_webhook_delivers_turn_end_event() {
    let _ = tracing_subscriber::fmt().try_init();

    // 1. Start a webhook receiver (wiremock)
    let webhook_server = MockServer::start().await;
    let webhook_port = webhook_server
        .uri()
        .trim_start_matches("http://127.0.0.1:")
        .parse::<u16>()
        .unwrap();

    Mock::given(method("POST"))
        .and(path("/webhook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&webhook_server)
        .await;

    // 2. Start LLM provider
    let body = common::openai_text_sse_body("webhook test");
    let (_llm_server, provider) = common::start_wiremock_openai(&body).await;

    let webhook_url = format!("http://mock-webhook.test:{}/webhook", webhook_port);

    // Build a custom HTTP client that resolves mock-webhook.test to the wiremock addr.
    let http_client = reqwest::Client::builder()
        .no_proxy()
        .resolve(
            "mock-webhook.test",
            format!("127.0.0.1:{}", webhook_port).parse().unwrap(),
        )
        .build()
        .unwrap();

    // Verify resolve works before building app
    let test_resp = http_client.post(&webhook_url).body("test").send().await;
    println!("test_resp: {:?}", test_resp);

    let app = common::build_test_app_with_client(provider, http_client);
    let token = common::make_token("test-tenant");
    let create_body = serde_json::json!({
        "title": "webhook test",
        "webhook": {
            "url": webhook_url,
            "events": ["turn_end"],
            "secret": "webhook-secret"
        }
    });

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(create_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_body = common::json_body(create_response).await;
    let session_id = create_body["id"].as_str().unwrap();

    // 4. Send message to trigger a turn
    let _send_response = app
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

    // 5. Wait for webhook delivery (with timeout)
    let delivery_result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let requests = webhook_server.received_requests().await.unwrap();
            if let Some(req) = requests
                .into_iter()
                .find(|r| r.headers.get("X-Pandaria-Event").is_some())
            {
                return req;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await;

    assert!(delivery_result.is_ok(), "webhook delivery timed out");
    let request = delivery_result.unwrap();

    // Verify headers
    assert_eq!(
        request
            .headers
            .get("X-Pandaria-Event")
            .unwrap()
            .to_str()
            .unwrap(),
        "turn_end"
    );
    assert!(
        request.headers.contains_key("X-Pandaria-Delivery"),
        "expected X-Pandaria-Delivery header"
    );
    assert!(
        request.headers.contains_key("X-Pandaria-Signature"),
        "expected X-Pandaria-Signature header"
    );
    assert_eq!(
        request
            .headers
            .get("X-Pandaria-Session-Id")
            .unwrap()
            .to_str()
            .unwrap(),
        session_id
    );
    assert_eq!(
        request
            .headers
            .get("X-Pandaria-Tenant-Id")
            .unwrap()
            .to_str()
            .unwrap(),
        "test-tenant"
    );

    // Verify body is valid JSON with turn_end type
    let body_str = String::from_utf8(request.body.clone()).unwrap();
    let body_json: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(body_json["type"], "turn_end");
    assert!(body_json["delivery_id"].is_string());

    // Verify HMAC-SHA256 signature
    let signature_header = request
        .headers
        .get("X-Pandaria-Signature")
        .unwrap()
        .to_str()
        .unwrap();
    let expected_hmac = hmac_sha256("webhook-secret", &body_str);
    assert_eq!(
        signature_header,
        format!("sha256={}", expected_hmac),
        "HMAC signature mismatch"
    );
}

#[tokio::test]
async fn test_webhook_ssrf_blocked() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("webhook ssrf");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    let create_body = serde_json::json!({
        "title": "webhook ssrf test",
        "webhook": {
            "url": "http://127.0.0.1:9999/webhook",
            "events": ["turn_end"]
        }
    });

    let create_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(create_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_response.status(), StatusCode::BAD_REQUEST);
}

fn hmac_sha256(secret: &str, body: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(body.as_bytes());
    let result = mac.finalize();
    let bytes = result.into_bytes();
    hex::encode(bytes)
}
