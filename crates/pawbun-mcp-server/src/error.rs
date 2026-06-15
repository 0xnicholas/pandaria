use pawbun_toolkit::mcp::TransportError;
use pawbun_toolkit::ToolError;
use pawbun_files::LoadError;

/// Errors that can occur in MCP Server operations.
#[derive(thiserror::Error, Debug)]
pub enum McpServerError {
    /// Transport layer error.
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),

    /// Tool execution error.
    #[error("tool error: {0}")]
    Tool(#[from] ToolError),

    /// File loading error.
    #[error("load error: {0}")]
    Load(#[from] LoadError),

    /// Server bind error.
    #[error("bind failed: {0}")]
    Bind(String),

    /// MCP protocol error.
    #[error("MCP protocol error: {message} (code {code})")]
    Protocol {
        /// Error message.
        message: String,
        /// Error code.
        code: i32,
    },
}
