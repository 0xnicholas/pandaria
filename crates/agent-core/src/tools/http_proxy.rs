//! HTTP proxy tool that forwards tool_call invocations to an external HTTP endpoint.
//!
//! Each session can be configured with a set of `HttpProxyTool`s, allowing
//! external orchestrators to inject custom tool capabilities without rebuilding
//! the runtime.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::types::{AgentTool, AgentToolProgressUpdate, AgentToolResult};
use crate::utils::ssrf::is_internal_endpoint;

/// Configuration for an external HTTP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    /// Tool name, used as the tool_call identifier.
    pub name: String,
    /// Human-readable description injected into the LLM system prompt.
    pub description: String,
    /// JSON Schema describing the tool parameters.
    pub parameters: serde_json::Value,
    /// HTTP endpoint for tool execution.
    pub endpoint: String,
    /// Request timeout in milliseconds (default: 30000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Optional authentication headers.
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
}

impl ToolConfig {
    /// Returns the timeout duration, defaulting to 30 seconds.
    fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms.unwrap_or(30_000))
    }
}

/// An `AgentTool` implementation that forwards tool calls to an external HTTP endpoint.
pub struct HttpProxyTool {
    config: ToolConfig,
    tenant_id: String,
    session_id: String,
    client: reqwest::Client,
}

impl HttpProxyTool {
    /// Create a new `HttpProxyTool`.
    pub fn new(
        config: ToolConfig,
        tenant_id: String,
        session_id: String,
        client: reqwest::Client,
    ) -> Self {
        Self {
            config,
            tenant_id,
            session_id,
            client,
        }
    }
}

/// Request body sent to the external tool endpoint.
#[derive(Debug, Serialize)]
struct ToolRequestBody<'a> {
    tool_call_id: &'a str,
    params: serde_json::Value,
    session_id: &'a str,
    tenant_id: &'a str,
}

/// Response body expected from the external tool endpoint.
#[derive(Debug, Deserialize)]
struct ToolResponseBody {
    #[serde(default)]
    content: Vec<ai_provider::Content>,
    #[serde(default)]
    details: Option<serde_json::Value>,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    terminate: bool,
}

#[async_trait::async_trait]
impl AgentTool for HttpProxyTool {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn description(&self) -> &str {
        &self.config.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.config.parameters.clone()
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
        signal: CancellationToken,
    ) -> Result<AgentToolResult, AgentError> {
        // 1. SSRF guard
        if is_internal_endpoint(&self.config.endpoint) {
            return Ok(AgentToolResult {
                content: vec![ai_provider::Content::Text {
                    text: "SSRF: internal endpoint forbidden".into(),
                    text_signature: None,
                }],
                details: None,
                is_error: true,
                terminate: false,
            });
        }

        self.execute_inner(tool_call_id, params, on_progress, signal)
            .await
    }
}

impl HttpProxyTool {
    /// The actual HTTP request logic, separated so tests can exercise it
    /// without triggering the SSRF guard (wiremock binds to 127.0.0.1).
    async fn execute_inner(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
        signal: CancellationToken,
    ) -> Result<AgentToolResult, AgentError> {
        // 2. Build request
        let body = ToolRequestBody {
            tool_call_id,
            params,
            session_id: &self.session_id,
            tenant_id: &self.tenant_id,
        };

        let mut req = self
            .client
            .post(&self.config.endpoint)
            .json(&body)
            .timeout(self.config.timeout());

        if let Some(ref headers) = self.config.headers {
            for (k, v) in headers {
                req = req.header(k, v);
            }
        }

        // 3. Execute with cancellation support
        let resp = tokio::select! {
            biased;
            _ = signal.cancelled() => {
                return Ok(AgentToolResult {
                    content: vec![ai_provider::Content::Text {
                        text: "Tool call cancelled".into(),
                        text_signature: None,
                    }],
                    details: None,
                    is_error: true,
                    terminate: false,
                });
            }
            result = req.send() => result,
        };

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                return Ok(AgentToolResult {
                    content: vec![ai_provider::Content::Text {
                        text: format!("HTTP error: {}", e),
                        text_signature: None,
                    }],
                    details: Some(serde_json::json!({ "error": e.to_string() })),
                    is_error: true,
                    terminate: false,
                });
            }
        };

        // 4. Parse response
        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Ok(AgentToolResult {
                content: vec![ai_provider::Content::Text {
                    text: format!("HTTP {}: {}", status, body_text),
                    text_signature: None,
                }],
                details: Some(serde_json::json!({ "status": status.as_u16() })),
                is_error: true,
                terminate: false,
            });
        }

        let body: ToolResponseBody = match resp.json().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(AgentToolResult {
                    content: vec![ai_provider::Content::Text {
                        text: format!("Failed to parse tool response: {}", e),
                        text_signature: None,
                    }],
                    details: None,
                    is_error: true,
                    terminate: false,
                });
            }
        };

        Ok(AgentToolResult {
            content: body.content,
            details: body.details,
            is_error: body.is_error,
            terminate: body.terminate,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_tool(endpoint: String) -> HttpProxyTool {
        HttpProxyTool::new(
            ToolConfig {
                name: "test_tool".into(),
                description: "A test tool".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    }
                }),
                endpoint,
                timeout_ms: Some(5000),
                headers: None,
            },
            "tenant-1".into(),
            "session-1".into(),
            reqwest::Client::new(),
        )
    }

    #[tokio::test]
    async fn test_ssrf_blocked() {
        // Use 10.0.0.1 (private range, not a local bindable address) so the
        // SSRF guard fires without needing a real server.
        let tool = make_tool("http://10.0.0.1:9999/invoke".into());
        let result = tool
            .execute(
                "call_001",
                serde_json::json!({"query": "hello"}),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert_eq!(content_text(&result), "SSRF: internal endpoint forbidden");
    }

    #[tokio::test]
    async fn test_successful_proxy() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/invoke"))
            .and(body_json(serde_json::json!({
                "tool_call_id": "call_001",
                "params": { "query": "hello" },
                "session_id": "session-1",
                "tenant_id": "tenant-1",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "result"}],
                "details": { "extra": 42 },
                "is_error": false,
                "terminate": false,
            })))
            .mount(&server)
            .await;

        let tool = make_tool(server.uri() + "/invoke");
        let result = tool
            .execute_inner(
                "call_001",
                serde_json::json!({"query": "hello"}),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(!result.terminate);
        assert_eq!(content_text(&result), "result");
        assert_eq!(result.details, Some(serde_json::json!({ "extra": 42 })));
    }

    #[tokio::test]
    async fn test_non_2xx_response() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/invoke"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&server)
            .await;

        let tool = make_tool(server.uri() + "/invoke");
        let result = tool
            .execute_inner(
                "call_001",
                serde_json::json!({}),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        let text = content_text(&result);
        assert!(text.contains("HTTP 500"));
        assert!(text.contains("Internal Server Error"));
    }

    #[tokio::test]
    async fn test_cancellation() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/slow"))
            .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(10)))
            .mount(&server)
            .await;

        let tool = make_tool(server.uri() + "/slow");
        let token = CancellationToken::new();
        let token_clone = token.clone();

        let handle = tokio::spawn(async move {
            tool.execute_inner("call_001", serde_json::json!({}), None, token_clone)
                .await
        });

        token.cancel();
        let result = handle.await.unwrap().unwrap();

        assert!(result.is_error);
        assert_eq!(content_text(&result), "Tool call cancelled");
    }

    #[tokio::test]
    async fn test_custom_headers() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/invoke"))
            .and(header("X-Custom-Header", "secret-value"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "ok"}],
                "is_error": false,
            })))
            .mount(&server)
            .await;

        let mut tool = make_tool(server.uri() + "/invoke");
        tool.config.headers = Some({
            let mut h = HashMap::new();
            h.insert("X-Custom-Header".into(), "secret-value".into());
            h
        });

        let result = tool
            .execute_inner(
                "call_001",
                serde_json::json!({}),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(content_text(&result), "ok");
    }

    #[tokio::test]
    async fn test_terminate_propagation() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/invoke"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "done"}],
                "is_error": false,
                "terminate": true,
            })))
            .mount(&server)
            .await;

        let tool = make_tool(server.uri() + "/invoke");
        let result = tool
            .execute_inner(
                "call_001",
                serde_json::json!({}),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(result.terminate);
    }

    fn content_text(result: &AgentToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|c| match c {
                ai_provider::Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}
