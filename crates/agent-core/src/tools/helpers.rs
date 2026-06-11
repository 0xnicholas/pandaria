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
    use crate::test_utils::TestTool;
    use std::sync::Arc;

    #[test]
    fn test_build_tool_defs_empty() {
        let defs = build_tool_defs(&[]);
        assert!(defs.is_none());
    }

    #[test]
    fn test_build_tool_defs_non_empty() {
        let tool: AgentToolRef = Arc::new(TestTool::new(
            "echo",
            "Echoes input",
            serde_json::json!({"type": "object"}),
        ));
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
        let tool: AgentToolRef = Arc::new(TestTool::new(
            "grep",
            "Searches files",
            serde_json::json!({"type": "object"}),
        ));
        let defs = build_tool_value_defs(&[tool]);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["name"], "grep");
        assert_eq!(defs[0]["description"], "Searches files");
    }
}
