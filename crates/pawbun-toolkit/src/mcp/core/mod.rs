//! MCP protocol core types and transport abstractions.
//!
//! This module was migrated from the former `pawbun-mcp-core` crate into
//! `pawbun-toolkit` to reduce workspace fragmentation.

/// MCP JSON-RPC 2.0 protocol types.
pub mod protocol;
/// Bidirectional schema conversion between MCP input schema and ToolParameter lists.
pub mod schema_convert;
/// Transport abstractions for MCP communication (traits and configs, not implementations).
pub mod transport;
