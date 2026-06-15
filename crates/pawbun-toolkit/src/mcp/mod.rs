//! MCP (Model Context Protocol) adapter layer.
//!
//! Provides connectivity to MCP servers via stdio or SSE transport,
//! and exposes remote MCP tools as local `Tool` trait implementations.
//!
//! # Example
//! ```no_run
//! use pawbun_toolkit::mcp::{McpAdapter, TransportConfig};
//!
//! let config = TransportConfig::Stdio {
//!     command: "npx".into(),
//!     args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into(), "/tmp".into()],
//! };
//! let mut session = McpAdapter::connect(config).unwrap();
//! let tools = session.list_tools().unwrap();
//! ```

pub mod adapter;
/// MCP protocol core types (protocol, schema_convert, transport traits).
pub mod core;
pub mod dynamic_tool;
pub mod transport;

// Re-export core types for backward compatibility
pub use self::core::protocol::*;
pub use self::core::schema_convert::*;
pub use self::core::transport::{
    ServerTransport, ServerTransportConfig, Transport, TransportConfig, TransportError,
};

pub use adapter::{McpAdapter, McpError, McpSession};
pub use dynamic_tool::DynamicTool;
pub use transport::{StdioTransport, SseTransport};
