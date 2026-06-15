//! Custom input struct with `#[pawbun_tool]` macro example.
//!
//! Demonstrates a macro-generated tool that accepts a custom input structure.

use pawbun_toolkit::{Tool, ToolError, ToolResult, ToolParameter};
use pawbun_toolkit_macros::pawbun_tool;
use serde_json::json;
use std::borrow::Cow;

#[derive(Debug)]
struct AddTool;

#[pawbun_tool(name = "add", description = "Add two numbers")]
impl Tool for AddTool {
    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "a".into(),
                description: "First number".into(),
                required: true,
                schema: json!({"type": "number"}),
            },
            ToolParameter {
                name: "b".into(),
                description: "Second number".into(),
                required: true,
                schema: json!({"type": "number"}),
            },
        ])
    }

    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value = serde_json::from_str(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid JSON: {e}")))?;
        let a = parsed.get("a").and_then(|v| v.as_f64())
            .ok_or_else(|| ToolError::invalid_input("missing 'a' field"))?;
        let b = parsed.get("b").and_then(|v| v.as_f64())
            .ok_or_else(|| ToolError::invalid_input("missing 'b' field"))?;
        Ok(ToolResult {
            success: true,
            content: format!("{}", a + b),
            metadata: None,
            elapsed_ms: None,
        })
    }
}

fn main() {
    let tool = AddTool;
    println!("Name: {}", tool.name());
    println!("Description: {}", tool.description());

    let result = tool.execute(r#"{"a": 3.14, "b": 2.86}"#).unwrap();
    println!("3.14 + 2.86 = {}", result.content);
}
