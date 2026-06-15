//! Basic `#[pawbun_tool]` macro example.
//!
//! Demonstrates using the macro to define a tool with minimal boilerplate.

use pawbun_toolkit::{Tool, ToolError, ToolResult};
use pawbun_toolkit_macros::pawbun_tool;

#[derive(Debug)]
struct EchoTool;

#[pawbun_tool(name = "echo", description = "Echoes the input back")]
impl Tool for EchoTool {
    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            success: true,
            content: input.into(),
            metadata: None,
            elapsed_ms: None,
        })
    }
}

fn main() {
    let tool = EchoTool;
    println!("Name: {}", tool.name());
    println!("Description: {}", tool.description());
    println!("Parameters: {:?}", tool.parameters());

    let result = tool.execute("hello world").unwrap();
    println!("Result: {}", result.content);
}
