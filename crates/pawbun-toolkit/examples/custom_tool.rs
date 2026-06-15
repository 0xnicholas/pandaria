//! Custom Tool implementation example.
//!
//! Demonstrates implementing the Tool trait manually without macros.

use pawbun_toolkit::{Tool, ToolError, ToolExecutor, ToolParameter, ToolResult};
use serde_json::json;
use std::borrow::Cow;

#[derive(Debug)]
struct GreetTool;

impl Tool for GreetTool {
    fn name(&self) -> &str { "greet" }
    fn description(&self) -> &str { "Greet someone by name." }
    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "name".into(),
                description: "Name to greet".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
        ])
    }
    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value = serde_json::from_str(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid JSON: {e}")))?;
        let name = parsed.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'name' field"))?;
        Ok(ToolResult {
            success: true,
            content: format!("Hello, {name}!"),
            metadata: None,
            elapsed_ms: None,
        })
    }
}

fn main() {
    let mut toolkit = pawbun_toolkit::ToolKit::new();
    toolkit.register(Box::new(GreetTool));
    let result = toolkit.execute("greet", r#"{"name": "Pawbun"}"#).unwrap();
    println!("{}", result.content);
}
