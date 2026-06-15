//! Stdio MCP server example.
//!
//! Demonstrates creating an MCP server that communicates over stdin/stdout.

use pawbun_mcp_server::McpServer;
use pawbun_toolkit::mcp::ServerTransportConfig;
use pawbun_toolkit::{ToolKit, FileReadTool};
use pawbun_files::DefaultFileLoader;

fn main() {
    let mut toolkit = ToolKit::new();
    toolkit.register(Box::new(FileReadTool::default()));

    let loader = DefaultFileLoader::with_base_dir("/app/data");

    let server = McpServer::builder("pawbun-stdio", "0.1.0")
        .register_toolkit(toolkit)
        .register_file_loader(loader)
        .build();

    println!("Starting stdio MCP server...");
    server.launch(ServerTransportConfig::Stdio).unwrap();
}
