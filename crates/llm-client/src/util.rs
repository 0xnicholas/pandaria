use crate::types::{Content, ToolCall, ToolDef};

/// Build a Vec<ToolDef> from a tool definition slice.
///
/// This is semantically equivalent to `tools.to_vec()`, but provides
/// a named function for clarity at call sites.
pub fn build_tool_defs(tools: &[ToolDef]) -> Vec<ToolDef> {
    tools.to_vec()
}

/// Extract all ToolCall entries from content blocks.
pub fn extract_tool_calls(content: &[Content]) -> Vec<ToolCall> {
    content
        .iter()
        .filter_map(|c| match c {
            Content::ToolCall(tc) => Some(tc.clone()),
            _ => None,
        })
        .collect()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tool_calls_empty() {
        let content = vec![Content::Text {
            text: "hi".to_string(),
            text_signature: None,
        }];
        assert!(extract_tool_calls(&content).is_empty());
    }

    #[test]
    fn test_extract_tool_calls_found() {
        let tc = ToolCall {
            id: "c1".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        };
        let content = vec![
            Content::Text {
                text: "ok".to_string(),
                text_signature: None,
            },
            Content::ToolCall(tc.clone()),
        ];
        let calls = extract_tool_calls(&content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read");
    }


}
