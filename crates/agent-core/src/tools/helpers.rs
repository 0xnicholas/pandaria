use crate::tools::AgentToolRef;

/// Build the LLM function-definition list from a tool set.
///
/// Returns `None` when `tools` is empty, preserving the semantic that
/// an empty tool list means "no function calling capabilities".
pub fn build_tool_defs(tools: &[AgentToolRef]) -> Option<Vec<ai_provider::ToolDef>> {
    if tools.is_empty() {
        None
    } else {
        Some(
            tools
                .iter()
                .map(|t| ai_provider::ToolDef {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: t.parameters(),
                })
                .collect(),
        )
    }
}

/// Build `serde_json::Value` representations of the tool set for hook contexts.
pub fn build_tool_value_defs(tools: &[AgentToolRef]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name(),
                "description": t.description(),
                "parameters": t.parameters(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AgentError;
    use crate::tools::{AgentTool, AgentToolProgressUpdate, AgentToolResult};
    use ai_provider::Content;
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    struct MockTool {
        name: &'static str,
        desc: &'static str,
        params: serde_json::Value,
    }

    #[async_trait]
    impl AgentTool for MockTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            self.desc
        }
        fn parameters(&self) -> serde_json::Value {
            self.params.clone()
        }
        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
            _signal: CancellationToken,
        ) -> Result<AgentToolResult, AgentError> {
            Ok(AgentToolResult {
                content: vec![],
                details: None,
                is_error: false,
                terminate: false,
            })
        }
    }

    #[test]
    fn test_build_tool_defs_empty() {
        let defs = build_tool_defs(&[]);
        assert!(defs.is_none());
    }

    #[test]
    fn test_build_tool_defs_non_empty() {
        let tool: AgentToolRef = Arc::new(MockTool {
            name: "echo",
            desc: "Echoes input",
            params: serde_json::json!({"type": "object"}),
        });
        let defs = build_tool_defs(&[tool]).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "echo");
        assert_eq!(defs[0].description, "Echoes input");
    }

    #[test]
    fn test_build_tool_value_defs_empty() {
        let defs = build_tool_value_defs(&[]);
        assert!(defs.is_empty());
    }

    #[test]
    fn test_build_tool_value_defs_non_empty() {
        let tool: AgentToolRef = Arc::new(MockTool {
            name: "grep",
            desc: "Searches files",
            params: serde_json::json!({"type": "object"}),
        });
        let defs = build_tool_value_defs(&[tool]);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["name"], "grep");
        assert_eq!(defs[0]["description"], "Searches files");
    }
}
