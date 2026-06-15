//! Basic ToolKit usage example.
//!
//! Demonstrates creating a ToolKit, registering built-in tools, and executing them.

use pawbun_toolkit::{ToolKit, ToolRegistry, ToolExecutor, FileReadTool};

fn main() {
    let mut toolkit = ToolKit::new();
    toolkit.register(Box::new(FileReadTool::default()));

    println!("Registered tools: {}", toolkit.len());
    println!("Available tools:\n{}", toolkit.descriptions());

    // Execute file_read tool (will fail if README.md doesn't exist)
    match toolkit.execute("file_read", r#"{"path": "README.md"}"#) {
        Ok(result) => println!("Content preview: {}", &result.content[..result.content.len().min(200)]),
        Err(e) => eprintln!("Error: {}", e),
    }
}
