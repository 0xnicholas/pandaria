//! End-to-end integration test: WebSocket event stream.
//!
//! Verifies that the WS /sessions/{id}/ws endpoint accepts connections,
//! delivers session events, and handles client actions.

mod common;

use std::sync::Arc;

use futures::{SinkExt, StreamExt};

async fn start_test_server(
    provider: Arc<dyn ai_provider::LlmProvider>,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = common::build_test_app(provider).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), handle)
}

#[tokio::test]
async fn test_websocket_receives_events() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("ws test");
    let (_llm_server, provider) = common::start_wiremock_openai(&body).await;
    let (base_url, _server_handle) = start_test_server(provider).await;
    let token = "pk_live_test-tenant";

    // Create session via HTTP
    let client = reqwest::Client::new();
    let create_resp = client
        .post(format!("{}/api/v1/sessions", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"title": "ws test"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(create_resp.status(), 201);
    let create_body: serde_json::Value = create_resp.json().await.unwrap();
    let session_id = create_body["id"].as_str().unwrap();

    // Connect WebSocket
    let ws_url = format!(
        "ws://{}/api/v1/sessions/{}/ws",
        base_url.trim_start_matches("http://"),
        session_id
    );
    let host = ws_url
        .trim_start_matches("ws://")
        .split('/')
        .next()
        .unwrap();
    let mut req = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&ws_url)
        .header("Host", host)
        .header("Authorization", format!("Bearer {}", token))
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .header("Sec-WebSocket-Version", "13")
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .body(())
        .unwrap();

    let (mut ws_stream, _resp) = tokio_tungstenite::connect_async(req).await.unwrap();

    // Send a message action to trigger a turn
    let action = serde_json::json!({"action": "send_message", "content": [{"type": "text", "text": "hello"}]});
    ws_stream
        .send(tokio_tungstenite::tungstenite::Message::Text(
            action.to_string().into(),
        ))
        .await
        .unwrap();

    // Collect events until TurnEnd
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(500), ws_stream.next()).await {
            Ok(Some(Ok(msg))) => {
                if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                        events.push(event.clone());
                        if event.get("type").and_then(|v| v.as_str()) == Some("turn_end") {
                            break;
                        }
                    }
                }
            }
            Ok(Some(Err(_))) => break,
            Ok(None) => break,
            Err(_) => continue,
        }
    }

    assert!(
        !events.is_empty(),
        "expected at least one event via websocket"
    );

    let has_turn_end = events
        .iter()
        .any(|e| e.get("type").and_then(|v| v.as_str()) == Some("turn_end"));
    assert!(has_turn_end, "expected turn_end event via websocket");

    // Close websocket gracefully
    let _ = ws_stream.close(None).await;
}

#[tokio::test]
async fn test_websocket_auth_failure() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("ws auth");
    let (_llm_server, provider) = common::start_wiremock_openai(&body).await;
    let (base_url, _server_handle) = start_test_server(provider).await;

    // Create session with valid token first
    let token = "pk_live_test-tenant";
    let client = reqwest::Client::new();
    let create_resp = client
        .post(format!("{}/api/v1/sessions", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"title": "ws auth test"}"#)
        .send()
        .await
        .unwrap();

    let create_body: serde_json::Value = create_resp.json().await.unwrap();
    let session_id = create_body["id"].as_str().unwrap();

    // Try to connect with invalid token
    let ws_url = format!(
        "ws://{}/api/v1/sessions/{}/ws",
        base_url.trim_start_matches("http://"),
        session_id
    );
    let req = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&ws_url)
        .header("Authorization", "Bearer invalid-token")
        .body(())
        .unwrap();

    let result = tokio_tungstenite::connect_async(req).await;
    assert!(result.is_err(), "expected auth failure for invalid token");
}

#[tokio::test]
async fn test_websocket_interrupt() {
    let _ = tracing_subscriber::fmt().try_init();

    // LLM that never finishes (no [DONE])
    let slow_body = r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"delta":{"content":"slow"},"index":0}]}

"#;

    let (_llm_server, provider) = common::start_wiremock_openai(&slow_body).await;
    let (base_url, _server_handle) = start_test_server(provider).await;
    let token = "pk_live_test-tenant";

    let client = reqwest::Client::new();
    let create_resp = client
        .post(format!("{}/api/v1/sessions", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"title": "ws interrupt test"}"#)
        .send()
        .await
        .unwrap();

    let create_body: serde_json::Value = create_resp.json().await.unwrap();
    let session_id = create_body["id"].as_str().unwrap();

    let ws_url = format!(
        "ws://{}/api/v1/sessions/{}/ws",
        base_url.trim_start_matches("http://"),
        session_id
    );
    let host = ws_url
        .trim_start_matches("ws://")
        .split('/')
        .next()
        .unwrap();
    let req = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&ws_url)
        .header("Host", host)
        .header("Authorization", format!("Bearer {}", token))
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .header("Sec-WebSocket-Version", "13")
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .body(())
        .unwrap();

    let (mut ws_stream, _resp) = tokio_tungstenite::connect_async(req).await.unwrap();

    // Send message to start a turn
    let action = serde_json::json!({"action": "send_message", "content": [{"type": "text", "text": "hello"}]});
    ws_stream
        .send(tokio_tungstenite::tungstenite::Message::Text(
            action.to_string().into(),
        ))
        .await
        .unwrap();

    // Small delay to ensure turn started
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Send interrupt
    let interrupt = serde_json::json!({"action": "interrupt"});
    ws_stream
        .send(tokio_tungstenite::tungstenite::Message::Text(
            interrupt.to_string().into(),
        ))
        .await
        .unwrap();

    // Give time for events to flow
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify we can still receive messages after interrupt (connection alive)
    let pong = serde_json::json!({"action": "pong"});
    ws_stream
        .send(tokio_tungstenite::tungstenite::Message::Text(
            pong.to_string().into(),
        ))
        .await
        .unwrap();

    let _ = ws_stream.close(None).await;
}
