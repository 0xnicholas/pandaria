//! End-to-end integration test: PathGuard blocks file access outside tenant workspace.
//!
//! Verifies that `DefaultHookDispatcher::on_tool_call` intercepts tool calls
//! with paths outside `AgentSpace::workspace_for(tenant_id)` and returns
//! `HookDecision::Block` before the tool is executed.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_core::AgentSpace;
use agent_core::harness::config::HookConfig;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// Build a test app with custom PathGuard configuration.
async fn build_app_with_path_guard(
    provider: Arc<dyn ai_provider::LlmProvider>,
    path_guard_fields: HashMap<String, Vec<String>>,
    scan_unknown: bool,
) -> axum::Router {
    let space = AgentSpace::from_env_or_default();
    let mut hook_config = HookConfig::default();
    hook_config.path_guard_fields = path_guard_fields;
    hook_config.path_guard_scan_unknown = scan_unknown;
    let harness_config = agent_core::HarnessConfig {
        provider: provider.clone(),
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are a helpful assistant.".to_string(),
        default_context_window: 128_000,
        store: None,
        media_provider: None,
        media_registry: None,
        http_client: reqwest::Client::new(),
        ssrf_policy: Arc::new(agent_core::utils::ssrf::SsrfPolicy::strict()),
        available_models: vec!["gpt-4".to_string()],
        compaction_config: agent_core::CompactionConfig::default(),
        agent_space: space,
        hook_config,
        memory_store: None,
        session_retention_days: 7,
        session_cleanup_interval_hours: 24,
    };
    common::build_test_app_with_config(provider, harness_config).await
}
/// OpenAI SSE body that emits a `read_file` tool call targeting `path`.
fn read_file_sse_body(path: &str) -> String {
    let args = serde_json::json!({"path": path}).to_string();
    format!(
        r#"data: {{"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{{"delta":{{"role":"assistant"}},"index":0}}]}}

data: {{"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{{"delta":{{"tool_calls":[{{"index":0,"id":"call_read1","function":{{"name":"read_file"}}}}]}},"index":0}}]}}

data: {{"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{{"delta":{{"tool_calls":[{{"index":0,"function":{{"arguments":"{}"}}}}]}},"index":0}}]}}

data: {{"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{{"delta":{{}},"finish_reason":"tool_calls","index":0}}]}}

data: [DONE]

"#,
        args.replace('"', "\\\"")
    )
}

/// Turn-2 SSE body: assistant text response after tool result.
fn text_after_tool_sse_body(text: &str) -> String {
    format!(
        r#"data: {{"id":"chatcmpl-t2","object":"chat.completion.chunk","choices":[{{"delta":{{"role":"assistant"}},"index":0}}]}}

data: {{"id":"chatcmpl-t2","object":"chat.completion.chunk","choices":[{{"delta":{{"content":"{}"}},"index":0}}]}}

data: {{"id":"chatcmpl-t2","object":"chat.completion.chunk","choices":[{{"delta":{{}},"finish_reason":"stop","index":0}}]}}

data: [DONE]

"#,
        text
    )
}

#[tokio::test]
async fn test_path_guard_blocks_file_outside_workspace() {
    let _ = tracing_subscriber::fmt().try_init();

    let mut path_fields = HashMap::new();
    path_fields.insert("read_file".to_string(), vec!["path".to_string()]);

    let turn1 = read_file_sse_body("/etc/passwd");
    let turn2 = text_after_tool_sse_body("blocked");

    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();
    let responder = move |_req: &wiremock::Request| {
        let c = cc.fetch_add(1, Ordering::SeqCst);
        if c == 0 {
            wiremock::ResponseTemplate::new(200).set_body_string(&turn1)
        } else {
            wiremock::ResponseTemplate::new(200).set_body_string(&turn2)
        }
    };

    let (_server, provider) = common::start_wiremock_openai_dynamic(responder).await;
    let app = build_app_with_path_guard(provider, path_fields, false).await;
    let token = "pk_live_test-tenant";

    // Create session with a registered "read_file" external tool
    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{
                    "title": "pathguard",
                    "tools": [{
                        "name": "read_file",
                        "endpoint": "https://httpbin.org/get",
                        "description": "Read a file",
                        "parameters": {
                            "type": "object",
                            "properties": { "path": { "type": "string" } },
                            "required": ["path"]
                        }
                    }]
                }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let sid = common::json_body(create).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Subscribe SSE
    let sse_app = app.clone();
    let sse_token = token.clone();
    let sse_sid = sid.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let sse_handle = tokio::spawn(async move {
        let resp = sse_app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/sessions/{}/events", sse_sid))
                    .header("Authorization", format!("Bearer {}", sse_token))
                    .header("Accept", "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let _ = ready_tx.send(());
        common::collect_sse_events(resp).await
    });
    ready_rx.await.unwrap();

    // Send message triggering tool call
    let send = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"content": [{"type":"text","text":"read /etc/passwd"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send.status(), StatusCode::OK);

    let events = tokio::time::timeout(std::time::Duration::from_secs(5), sse_handle)
        .await
        .unwrap()
        .unwrap();

    // ToolCallDone should be present with is_error=true (blocked by hook)
    let tool_done = events
        .iter()
        .find(|e| matches!(e, api_gateway::types::ServerEvent::ToolCallDone { .. }));
    assert!(tool_done.is_some(), "expected ToolCallDone");

    if let api_gateway::types::ServerEvent::ToolCallDone { is_error, .. } = tool_done.unwrap() {
        assert!(*is_error, "tool call should be blocked (is_error=true)");
    }

    // Verify message history contains blocked details
    let msgs_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let msgs = common::json_body(msgs_response).await;
    let tool_result = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m.get("tool_call_id").is_some() && m.get("details").is_some());
    assert!(tool_result.is_some(), "expected tool_result in history");
    let details = tool_result.unwrap()["details"].clone();
    assert!(
        details.get("blocked").is_some(),
        "details should contain blocked flag, got: {:?}",
        details
    );
    let reason = details["reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("path") || reason.contains("workspace"),
        "reason should mention path/workspace, got: {}",
        reason
    );

    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "expected 2 LLM calls (tool call + final response)"
    );
}

#[tokio::test]
async fn test_path_guard_allows_file_inside_workspace() {
    let _ = tracing_subscriber::fmt().try_init();

    let space = AgentSpace::from_env_or_default();
    let workspace = space.workspace_for("test-tenant");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    let safe_file = workspace.join("safe.txt");
    tokio::fs::write(&safe_file, "hello workspace")
        .await
        .unwrap();

    let mut path_fields = HashMap::new();
    path_fields.insert("read_file".to_string(), vec!["path".to_string()]);

    let turn1 = read_file_sse_body(safe_file.to_str().unwrap());
    let turn2 = text_after_tool_sse_body("ok");

    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();
    let responder = move |_req: &wiremock::Request| {
        let c = cc.fetch_add(1, Ordering::SeqCst);
        if c == 0 {
            wiremock::ResponseTemplate::new(200).set_body_string(&turn1)
        } else {
            wiremock::ResponseTemplate::new(200).set_body_string(&turn2)
        }
    };

    let (_server, provider) = common::start_wiremock_openai_dynamic(responder).await;
    let app = build_app_with_path_guard(provider, path_fields, false).await;
    let token = "pk_live_test-tenant";

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{
                    "title": "pathguard-safe",
                    "tools": [{
                        "name": "read_file",
                        "endpoint": "https://httpbin.org/get",
                        "description": "Read a file",
                        "parameters": {
                            "type": "object",
                            "properties": { "path": { "type": "string" } },
                            "required": ["path"]
                        }
                    }]
                }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = common::json_body(create).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let sse_app = app.clone();
    let sse_token = token.clone();
    let sse_sid = sid.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let sse_handle = tokio::spawn(async move {
        let resp = sse_app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/sessions/{}/events", sse_sid))
                    .header("Authorization", format!("Bearer {}", sse_token))
                    .header("Accept", "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let _ = ready_tx.send(());
        common::collect_sse_events(resp).await
    });
    ready_rx.await.unwrap();

    let send = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"content": [{"type":"text","text":"read safe.txt"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send.status(), StatusCode::OK);

    let events = tokio::time::timeout(std::time::Duration::from_secs(5), sse_handle)
        .await
        .unwrap()
        .unwrap();

    let tool_done = events
        .iter()
        .find(|e| matches!(e, api_gateway::types::ServerEvent::ToolCallDone { .. }));
    assert!(tool_done.is_some());

    // PathGuard allows the path, but read_file tool is not registered,
    // so it will still be an error — but NOT a blocked error.
    if let api_gateway::types::ServerEvent::ToolCallDone { is_error, .. } = tool_done.unwrap() {
        assert!(*is_error, "unregistered tool should also produce is_error");
    }

    // Verify message history: details should NOT contain "blocked" flag
    let msgs_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let msgs = common::json_body(msgs_response).await;
    let tool_result = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m.get("tool_call_id").is_some() && m.get("details").is_some());
    assert!(tool_result.is_some(), "expected tool_result in history");
    let details = tool_result.unwrap()["details"].clone();
    assert!(
        details.get("blocked").is_none(),
        "details should NOT contain blocked flag (path was allowed), got: {:?}",
        details
    );
}

#[tokio::test]
async fn test_path_guard_scan_unknown_blocks_illegal_paths() {
    let _ = tracing_subscriber::fmt().try_init();

    // No path_guard_fields for "custom_tool", but scan_unknown=true
    let path_fields: HashMap<String, Vec<String>> = HashMap::new();

    let args = serde_json::json!({"target": "/etc/shadow"}).to_string();
    let turn1 = format!(
        r#"data: {{"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{{"delta":{{"role":"assistant"}},"index":0}}]}}

data: {{"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{{"delta":{{"tool_calls":[{{"index":0,"id":"call_custom","function":{{"name":"custom_tool"}}}}]}},"index":0}}]}}

data: {{"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{{"delta":{{"tool_calls":[{{"index":0,"function":{{"arguments":"{}"}}}}]}},"index":0}}]}}

data: {{"id":"chatcmpl-t1","object":"chat.completion.chunk","choices":[{{"delta":{{}},"finish_reason":"tool_calls","index":0}}]}}

data: [DONE]

"#,
        args.replace('"', "\\\"")
    );
    let turn2 = text_after_tool_sse_body("done");

    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();
    let responder = move |_req: &wiremock::Request| {
        let c = cc.fetch_add(1, Ordering::SeqCst);
        if c == 0 {
            wiremock::ResponseTemplate::new(200).set_body_string(&turn1)
        } else {
            wiremock::ResponseTemplate::new(200).set_body_string(&turn2)
        }
    };

    let (_server, provider) = common::start_wiremock_openai_dynamic(responder).await;
    let app = build_app_with_path_guard(provider, path_fields, true).await;
    let token = "pk_live_test-tenant";

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{
                    "title": "scan-unknown",
                    "tools": [{
                        "name": "custom_tool",
                        "endpoint": "https://httpbin.org/get",
                        "description": "Custom tool",
                        "parameters": {
                            "type": "object",
                            "properties": { "target": { "type": "string" } },
                            "required": ["target"]
                        }
                    }]
                }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = common::json_body(create).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let sse_app = app.clone();
    let sse_token = token.clone();
    let sse_sid = sid.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let sse_handle = tokio::spawn(async move {
        let resp = sse_app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/sessions/{}/events", sse_sid))
                    .header("Authorization", format!("Bearer {}", sse_token))
                    .header("Accept", "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let _ = ready_tx.send(());
        common::collect_sse_events(resp).await
    });
    ready_rx.await.unwrap();

    let send = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"content": [{"type":"text","text":"run custom tool"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send.status(), StatusCode::OK);

    let events = tokio::time::timeout(std::time::Duration::from_secs(5), sse_handle)
        .await
        .unwrap()
        .unwrap();

    let tool_done = events
        .iter()
        .find(|e| matches!(e, api_gateway::types::ServerEvent::ToolCallDone { .. }));
    assert!(tool_done.is_some());

    if let api_gateway::types::ServerEvent::ToolCallDone { is_error, .. } = tool_done.unwrap() {
        assert!(
            *is_error,
            "unknown tool with illegal path should be blocked when scan_unknown=true"
        );
    }

    // Verify message history contains blocked details
    let msgs_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let msgs = common::json_body(msgs_response).await;
    let tool_result = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m.get("tool_call_id").is_some() && m.get("details").is_some());
    assert!(tool_result.is_some(), "expected tool_result in history");
    let details = tool_result.unwrap()["details"].clone();
    assert!(
        details.get("blocked").is_some(),
        "details should contain blocked flag, got: {:?}",
        details
    );
}
