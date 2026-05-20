//! End-to-end integration test: tool-use via HTTP API.
//!
//! Verifies that when the LLM emits `tool_calls`, the agent loop executes
//! (or fails to find) the tool and forwards `ToolCallDone` via SSE.
//!
//! Note: `TenantManagerImpl` currently does not register any custom tools, so
//! the tool call results in `is_error=true` with "tool not found". The test
//! verifies the event is still propagated through the SSE stream.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_tool_use_produces_sse_events() {
    let _ = tracing_subscriber::fmt().try_init();

    // Turn 1: assistant emits a tool call
    let turn1_body = r#"data: {"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}

data: {"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"echo"}}]},"index":0}]}

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
            wiremock::ResponseTemplate::new(200).set_body_string(turn1_body)
        } else {
            wiremock::ResponseTemplate::new(200).set_body_string(turn2_body)
        }
    };

    let (_server, provider) = common::start_wiremock_openai_dynamic(responder).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    // Create session
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "tool test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let create_body = common::json_body(create_response).await;
    let session_id = create_body["id"].as_str().unwrap();

    // Subscribe to SSE before sending the message.
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

    // Send message that triggers tool call
    let send_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"content": "call the echo tool"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(send_response.status(), StatusCode::OK);

    // Verify two LLM calls were made (tool call + final response)
    assert_eq!(call_count.load(Ordering::SeqCst), 2);

    // Collect SSE events
    let events = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        sse_handle,
    )
    .await
    .unwrap()
    .unwrap();

    // Verify ToolCallDone event is present
    let tool_done = events.iter().find(|e| {
        matches!(e, api_gateway::types::ServerEvent::ToolCallDone { .. })
    });
    assert!(tool_done.is_some(), "expected ToolCallDone in SSE events");

    if let api_gateway::types::ServerEvent::ToolCallDone { call_id, is_error, .. } = tool_done.unwrap() {
        assert_eq!(call_id, "call_abc");
        assert!(*is_error, "expected error because tool is not registered");
    }

    // Verify final TurnEnd
    let turn_end = events.iter().find(|e| {
        matches!(e, api_gateway::types::ServerEvent::TurnEnd { .. })
    });
    assert!(turn_end.is_some(), "expected TurnEnd in SSE events");

    // Verify message history contains user + assistant(tool) + tool_result + assistant(text)
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
        "expected at least 4 messages: user, assistant(tool), tool_result, assistant(text), got {}",
        msgs_arr.len()
    );

    // Verify tool_result message exists
    let has_tool_result = msgs_arr.iter().any(|m| {
        m.get("tool_call_id").is_some()
    });
    assert!(has_tool_result, "expected tool_result message in history");
}
