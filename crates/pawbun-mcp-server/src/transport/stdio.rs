//! Stdio server transport: reads JSON-RPC from stdin, writes responses to stdout.

use std::io::{BufRead, BufReader, Write};

use pawbun_toolkit::mcp::{JsonRpcRequest, JsonRpcResponse};
use pawbun_toolkit::mcp::{ServerTransport, TransportError};

/// Server transport using standard input/output.
///
/// Each JSON-RPC request is read as one line from stdin.
/// Each JSON-RPC response is written as one line to stdout.
///
/// Empty notification responses (id: null, result: null, error: null)
/// are silently suppressed to avoid confusing MCP clients.
pub struct StdioServerTransport {
    stdin: BufReader<std::io::Stdin>,
    stdout: std::io::Stdout,
}

impl StdioServerTransport {
    /// Creates a new stdio transport wrapping stdin/stdout.
    pub fn new() -> Self {
        Self {
            stdin: BufReader::new(std::io::stdin()),
            stdout: std::io::stdout(),
        }
    }
}

impl Default for StdioServerTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerTransport for StdioServerTransport {
    fn recv(&mut self) -> Result<JsonRpcRequest, TransportError> {
        let mut line = String::new();
        let n = self
            .stdin
            .read_line(&mut line)
            .map_err(|e| TransportError::Io {
                message: format!("failed to read from stdin: {e}"),
                kind: e.kind(),
            })?;
        if n == 0 {
            return Err(TransportError::UnexpectedEof);
        }
        serde_json::from_str(&line).map_err(|e| TransportError::Serialization(e.to_string()))
    }

    fn send(&mut self, resp: JsonRpcResponse) -> Result<(), TransportError> {
        // MCP spec: notification (id: null) does not expect a response.
        // The handler returns {jsonrpc, id: null, result: null, error: null} for
        // notifications/initialized. Suppress this empty response to avoid
        // confusing clients.
        let is_empty_notification =
            resp.id.is_none() && resp.result.is_none() && resp.error.is_none();
        if is_empty_notification {
            return Ok(());
        }

        let line = serde_json::to_string(&resp)
            .map_err(|e| TransportError::Serialization(e.to_string()))?;
        writeln!(self.stdout, "{}", line).map_err(|e| TransportError::Io {
            message: format!("failed to write to stdout: {e}"),
            kind: e.kind(),
        })?;
        self.stdout.flush().map_err(|e| TransportError::Io {
            message: format!("failed to flush stdout: {e}"),
            kind: e.kind(),
        })
    }

    fn close(self: Box<Self>) -> Result<(), TransportError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suppress_empty_notification_response() {
        let mut transport = StdioServerTransport::new();
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: None,
            result: None,
            error: None,
        };
        assert!(transport.send(resp).is_ok());
    }
}
