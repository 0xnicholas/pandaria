//! End-to-end integration test: external HTTP proxy tool via CreateSessionRequest.tools.
//!
//! Verifies that when a session is created with custom tools, the agent loop
//! forwards tool_call invocations to the external endpoint and returns the
//! response to the LLM.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_http_proxy_tool_end_to_end() {
    let _ = tracing_subscriber::fmt().try_init();

    // 1. Start an external tool endpoint (simulated by wiremock)
    let tool_server = MockServer::start().await;
    let tool_port = tool_server
        .uri()
        .trim_start_matches("http://127.0.0.1:")
        .parse::<u16>()
        .unwrap();

    Mock::given(method("POST"))
        .and(path("/invoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{"type": "text", "text": "pong"}],
            "is_error": false,
            "terminate": false,
        })))
        .mount(&tool_server)
        .await;

    // 2. Start the LLM provider that emits a tool call for "echo_proxy"
    // Turn 1: assistant emits tool_calls for echo_proxy
    let turn1_body = r#"data: {"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_proxy","function":{"name":"echo_proxy"}}]},"index":0}]}

data: {"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"message\":\"hello\"}"}}]},"index":0}]}

data: {"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"tool_calls","index":0}]}

data: [DONE]

"#;

    // Turn 2: assistant responds after receiving tool result
    let turn2_body = r#"data: {"id":"chatcmpl-t2","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-t2","object":"chat.completion.chunk","choices":[{"delta":{"content":"Done"},"index":0}]}

data: {"id":"chatcmpl-t2","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}

data: [DONE]

"#;

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let responder = move |_req: &wiremock::Request| {
        let count = call_count_clone.fetch_add(1, Ordering::SeqCst);
        if count == 0 {
            ResponseTemplate::new(200).set_body_string(turn1_body)
        } else {
            ResponseTemplate::new(200).set_body_string(turn2_body)
        }
    };

    let (_llm_server, provider) = common::start_wiremock_openai_dynamic(responder).await;

    // Build a custom HTTP client that resolves mock-tool.test to the wiremock addr.
    // This bypasses SSRF checks (mock-tool.test is not localhost) while still
    // routing requests to the local mock server.
    let http_client = reqwest::Client::builder()
        .no_proxy()
        .resolve(
            "mock-tool.test",
            format!("127.0.0.1:{}", tool_port).parse().unwrap(),
        )
        .build()
        .unwrap();

    let app = common::build_test_app_with_client(provider, http_client).await;
    let token = "pk_live_test-tenant";

    // 3. Create session with the external tool (use mock-tool.test to pass SSRF)
    let tool_endpoint = format!("http://mock-tool.test:{}/invoke", tool_port);
    let create_body = serde_json::json!({
        "title": "proxy tool test",
        "tools": [{
            "name": "echo_proxy",
            "description": "An external echo tool",
            "parameters": {
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                }
            },
            "endpoint": tool_endpoint,
            "timeout_ms": 5000,
        }]
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

    // 4. Subscribe SSE and send a message that triggers the tool call
    let sse_app = app.clone();
    let sse_token = token.clone();
    let sse_session_id = session_id.to_string();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

    let sse_handle = tokio::spawn(async move {
        let sse_response = sse_app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/sessions/{}/events", sse_session_id))
                    .header("Authorization", format!("Bearer {}", sse_token))
                    .header("Accept", "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let _ = ready_tx.send(());
        common::collect_sse_events(sse_response).await
    });

    ready_rx.await.expect("sse ready signal");

    let send_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"content": [{"type":"text","text":"call the echo_proxy tool"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(send_response.status(), StatusCode::OK);

    // 5. Verify two LLM calls were made
    assert_eq!(call_count.load(Ordering::SeqCst), 2);

    // 6. Collect SSE events and verify ToolCallDone is present and not an error
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), sse_handle)
        .await
        .unwrap()
        .unwrap();

    let tool_done = events
        .iter()
        .find(|e| matches!(e, api_gateway::types::ServerEvent::ToolCallDone { .. }));
    assert!(tool_done.is_some(), "expected ToolCallDone in SSE events");

    if let api_gateway::types::ServerEvent::ToolCallDone {
        call_id,
        is_error,
        result,
        ..
    } = tool_done.unwrap()
    {
        assert_eq!(call_id, "call_proxy");
        assert!(
            !is_error,
            "expected no error for proxy tool, got result: {:?}",
            result
        );
        assert_eq!(result.as_deref(), Some("pong"));
    }

    // 7. Verify message history contains user + assistant(tool) + tool_result + assistant(text)
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

    let msgs = common::json_body(msgs_response).await;
    let msgs_arr = msgs.as_array().unwrap();
    assert!(
        msgs_arr.len() >= 4,
        "expected at least 4 messages, got {}",
        msgs_arr.len()
    );

    // Verify tool_result message exists
    let has_tool_result = msgs_arr.iter().any(|m| m.get("tool_call_id").is_some());
    assert!(has_tool_result, "expected tool_result message in history");
}

#[tokio::test]
async fn test_http_proxy_tool_ssrf_blocked() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("ssrf test");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider).await;
    let token = "pk_live_test-tenant";

    // Try to create a session with an internal endpoint
    let create_body = serde_json::json!({
        "title": "ssrf test",
        "tools": [{
            "name": "bad_tool",
            "description": "A bad tool",
            "parameters": { "type": "object", "properties": {} },
            "endpoint": "http://127.0.0.1:9999/invoke",
        }]
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

    // SSRF is checked at creation time and returns 400
    assert_eq!(create_response.status(), StatusCode::BAD_REQUEST);
}
