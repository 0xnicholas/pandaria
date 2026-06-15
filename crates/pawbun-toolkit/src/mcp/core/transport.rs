//! MCP transport abstractions: client and server traits, configs, errors.

use std::io::ErrorKind;

use super::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Transport error.
#[derive(Debug, Clone, thiserror::Error)]
pub enum TransportError {
    /// IO error (file system, network, etc.).
    #[error("IO error: {message} (kind: {kind:?})")]
    Io {
        /// Error message.
        message: String,
        /// IO error kind.
        kind: ErrorKind,
    },
    /// Serialization or deserialization error.
    #[error("serialization error: {0}")]
    Serialization(String),
    /// Unexpected end of stream.
    #[error("unexpected EOF")]
    UnexpectedEof,
    /// HTTP error.
    #[error("HTTP error: {0}")]
    Http(String),
}

/// Client-side transport: sends a JSON-RPC request and blocks for a response.
pub trait Transport: Send + Sync {
    /// Sends a JSON-RPC request and waits for a response.
    fn request(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, TransportError>;

    /// Closes the transport connection.
    fn close(self: Box<Self>) -> Result<(), TransportError>;
}

/// Server-side transport: receives JSON-RPC requests and sends responses.
pub trait ServerTransport: Send {
    /// Blocking receive of the next JSON-RPC request.
    fn recv(&mut self) -> Result<JsonRpcRequest, TransportError>;

    /// Send a JSON-RPC response back to the client.
    fn send(&mut self, resp: JsonRpcResponse) -> Result<(), TransportError>;

    /// Graceful shutdown.
    fn close(self: Box<Self>) -> Result<(), TransportError>;
}

/// Client transport configuration.
#[derive(Debug, Clone)]
pub enum TransportConfig {
    /// Communicate via a subprocess's stdin/stdout.
    Stdio {
        /// Command to execute.
        command: String,
        /// Command arguments.
        args: Vec<String>,
    },
    /// Communicate via HTTP Server-Sent Events.
    Sse {
        /// SSE endpoint URL.
        url: String,
    },
}

/// Server transport configuration.
#[derive(Debug, Clone)]
pub enum ServerTransportConfig {
    /// Standard input/output (subprocess mode).
    Stdio,
    /// HTTP + SSE server (requires http feature).
    Sse {
        /// Bind address, e.g. "127.0.0.1:3000".
        bind_addr: String,
    },
}
