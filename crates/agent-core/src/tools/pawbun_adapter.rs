use std::sync::Arc;

use pawbun_toolkit::{Tool, ToolParameter, ToolResult as PToolResult};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::types::{AgentTool, AgentToolProgressUpdate, AgentToolResult};

/// Wraps a [`pawbun_toolkit::Tool`] as a Pandaria [`AgentTool`].
///
/// Converts:
/// - `ToolParameter[]` → JSON Schema (cached at construction)
/// - sync `execute(&str)` → async `execute` via `tokio::task::spawn_blocking`
/// - `ToolResult` → `AgentToolResult`
///
/// # Constraints
///
/// The Pawbun tool's sandbox base directory is **baked in at construction time**
/// via `AgentSpace::workspace_for(tenant_id)`. Per ADR-004, the tenant is
/// immutable for the session lifetime, so this is safe.
pub struct PawbunToolAdapter {
    inner: Arc<dyn Tool>,
    cached_schema: serde_json::Value,
    name: String,
    description: String,
}

impl PawbunToolAdapter {
    pub fn new(tool: Box<dyn Tool>) -> Self {
        let name = tool.name().to_string();
        let description = tool.description().to_string();
        let cached_schema = params_to_json_schema(&tool.parameters());
        Self {
            inner: Arc::from(tool),
            cached_schema,
            name,
            description,
        }
    }
}

/// Convert `ToolParameter[]` to a JSON Schema object value.
fn params_to_json_schema(params: &[ToolParameter]) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<serde_json::Value> = Vec::new();
    for p in params {
        properties.insert(p.name.clone(), p.schema.clone());
        if p.required {
            required.push(json!(p.name));
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// Convert a Pawbun `ToolResult` to an `AgentToolResult`.
fn pawbun_result_to_agent_result(r: PToolResult) -> Result<AgentToolResult, AgentError> {
    Ok(AgentToolResult {
        content: vec![ai_provider::Content::Text {
            text: r.content,
            text_signature: None,
        }],
        details: r.metadata,
        is_error: !r.success,
        terminate: false,
    })
}

#[async_trait::async_trait]
impl AgentTool for PawbunToolAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.cached_schema.clone()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
        signal: CancellationToken,
    ) -> Result<AgentToolResult, AgentError> {
        let input_json = serde_json::to_string(&params)
            .map_err(|e| AgentError::ToolExecutionFailed(format!("serialization: {e}")))?;

        // Branch: async tools (WebFetchTool, WebSearchTool) run directly
        // in the async context; sync tools run via spawn_blocking.
        if let Some(async_tool) = self.inner.as_async() {
            let input = input_json.clone();
            tokio::select! {
                result = async_tool.execute_async(&input) => {
                    match result {
                        Ok(tr) => pawbun_result_to_agent_result(tr),
                        Err(e) => Ok(AgentToolResult {
                            content: vec![ai_provider::Content::Text {
                                text: e.to_string(),
                                text_signature: None,
                            }],
                            details: None,
                            is_error: true,
                            terminate: false,
                        }),
                    }
                }
                _ = signal.cancelled() => {
                    Ok(AgentToolResult {
                        content: vec![ai_provider::Content::Text {
                            text: "cancelled".into(),
                            text_signature: None,
                        }],
                        details: None,
                        is_error: true,
                        terminate: false,
                    })
                }
            }
        } else {
            // Use Arc for 'static lifetime required by spawn_blocking
            let inner = Arc::clone(&self.inner);

            // NOTE: tokio::select! only stops *waiting* for the JoinHandle.
            // The blocking thread continues executing. CodeExecuteTool handles
            // actual cancellation via Child::kill(); file tools have resource
            // limits (max file size) to bound worst-case blocking time.
            tokio::select! {
                result = tokio::task::spawn_blocking(move || {
                    inner.execute(&input_json)
                }) => {
                    match result {
                        Ok(Ok(tr)) => pawbun_result_to_agent_result(tr),
                        Ok(Err(e)) => Ok(AgentToolResult {
                            content: vec![ai_provider::Content::Text {
                                text: e.to_string(),
                                text_signature: None,
                            }],
                            details: None,
                            is_error: true,
                            terminate: false,
                        }),
                        Err(join_err) => Ok(AgentToolResult {
                            content: vec![ai_provider::Content::Text {
                                text: format!("tool panicked: {join_err}"),
                                text_signature: None,
                            }],
                            details: None,
                            is_error: true,
                            terminate: false,
                        }),
                    }
                }
                _ = signal.cancelled() => {
                    Ok(AgentToolResult {
                        content: vec![ai_provider::Content::Text {
                            text: "cancelled".into(),
                            text_signature: None,
                        }],
                        details: None,
                        is_error: true,
                        terminate: false,
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawbun_toolkit::{ToolParameter, ToolResult as PToolResult, ToolError as PToolError};
    use serde_json::json;
    use std::borrow::Cow;

    #[derive(Debug)]
    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes input"
        }
        fn parameters(&self) -> Cow<'static, [ToolParameter]> {
            Cow::Owned(vec![ToolParameter {
                name: "message".into(),
                description: "Thing to echo".into(),
                required: true,
                schema: json!({"type": "string"}),
            }])
        }
        fn execute(&self, input: &str) -> Result<PToolResult, PToolError> {
            Ok(PToolResult {
                success: true,
                content: format!("echo: {}", input),
                metadata: Some(json!({"parsed": true})),
                elapsed_ms: None,
            })
        }
    }

    #[test]
    fn test_schema_conversion() {
        let adapter = PawbunToolAdapter::new(Box::new(EchoTool));
        let schema = adapter.parameters();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["message"]["type"], "string");
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("message"))
        );
    }

    #[test]
    fn test_schema_cached() {
        let adapter = PawbunToolAdapter::new(Box::new(EchoTool));
        let a = adapter.parameters();
        let b = adapter.parameters();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn test_execute_success() {
        let adapter = PawbunToolAdapter::new(Box::new(EchoTool));
        let result = adapter
            .execute(
                "call_1",
                json!({"message": "hello"}),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        let text = result
            .content
            .iter()
            .filter_map(|c| match c {
                ai_provider::Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(text, "echo: {\"message\":\"hello\"}");
        assert_eq!(result.details.unwrap()["parsed"], true);
    }

    #[tokio::test]
    async fn test_execute_tool_error() {
        #[derive(Debug)]
        struct FailingTool;
        impl Tool for FailingTool {
            fn name(&self) -> &str {
                "fail"
            }
            fn description(&self) -> &str {
                "Always fails"
            }
            fn parameters(&self) -> Cow<'static, [ToolParameter]> {
                Cow::Owned(vec![])
            }
            fn execute(&self, _input: &str) -> Result<PToolResult, PToolError> {
                Err(PToolError::invalid_input("bad input"))
            }
        }

        let adapter = PawbunToolAdapter::new(Box::new(FailingTool));
        let result = adapter
            .execute("call_1", json!({}), None, CancellationToken::new())
            .await
            .unwrap();

        assert!(result.is_error);
        let text = content_text(&result);
        assert!(text.contains("bad input"));
    }

    #[tokio::test]
    async fn test_execute_cancelled() {
        let adapter = PawbunToolAdapter::new(Box::new(EchoTool));
        let token = CancellationToken::new();
        token.cancel();

        let result = adapter
            .execute("call_1", json!({"message": "hi"}), None, token)
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(content_text(&result).contains("cancelled"));
    }

    fn content_text(result: &AgentToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|c| match c {
                ai_provider::Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }
}
