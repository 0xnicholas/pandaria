//! SSE MCP server example.
//!
//! Demonstrates creating an MCP server that communicates over SSE.
//! Requires the `http` feature.

use pawbun_mcp_server::McpServer;
use pawbun_toolkit::{ToolKit, FileReadTool};
use pawbun_files::DefaultFileLoader;

fn main() {
    let mut toolkit = ToolKit::new();
    toolkit.register(Box::new(FileReadTool::default()));

    let loader = DefaultFileLoader::with_base_dir("/app/data");

    let server = McpServer::builder("pawbun-sse", "0.1.0")
        .register_toolkit(toolkit)
        .register_file_loader(loader)
        .cors_origins(vec!["http://localhost:3000".into()])
        .build();

    println!("Starting SSE MCP server on http://127.0.0.1:3000");
    println!("GET /sse   — connect to SSE stream");
    println!("POST /message?sessionId=xxx — send JSON-RPC requests");
    server.launch(pawbun_toolkit::mcp::ServerTransportConfig::Sse {
        bind_addr: "127.0.0.1:3000".into(),
    }).unwrap();
}
