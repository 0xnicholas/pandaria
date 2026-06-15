#![deny(missing_docs)]
//! MCP Server for exposing Pawbun tools via Model Context Protocol.
//!
//! # Quick Start
//!
//! ```no_run
//! use pawbun_mcp_server::McpServer;
//! use pawbun_toolkit::mcp::ServerTransportConfig;
//! use pawbun_toolkit::{ToolKit, FileReadTool};
//! use pawbun_files::DefaultFileLoader;
//!
//! let mut toolkit = ToolKit::new();
//! toolkit.register(Box::new(FileReadTool::default()));
//!
//! let loader = DefaultFileLoader::with_base_dir("/app/data");
//!
//! let server = McpServer::builder("pawbun", "0.1.0")
//!     .register_toolkit(toolkit)
//!     .register_file_loader(loader)
//!     .build();
//!
//! // Blocking stdio server
//! server.launch(ServerTransportConfig::Stdio).unwrap();
//! ```

/// Server capability definitions.
pub mod capabilities;
/// Error types for MCP server operations.
pub mod error;
/// MCP request handler with initialization state machine.
pub mod handler;
/// MCP server and builder.
pub mod server;
/// Internal tool bridge (file loader integration).
pub(crate) mod tool_bridge;
/// Transport implementations (stdio, SSE).
pub mod transport;

pub use capabilities::*;
pub use error::McpServerError;
pub use server::{McpServer, McpServerBuilder};
