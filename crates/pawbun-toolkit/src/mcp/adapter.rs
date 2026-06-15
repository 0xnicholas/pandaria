//! MCP adapter and session management.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::json;

use super::core::protocol::{
    CallToolParams, CallToolResult, ClientInfo, InitializeParams, JsonRpcRequest, JsonRpcResponse,
    ListToolsResult, McpToolDesc,
};
use super::core::transport::{Transport, TransportConfig, TransportError};
use super::transport::StdioTransport;

/// Error type for MCP operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum McpError {
    /// Transport layer error (I/O, network, etc.).
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),
    /// JSON-RPC protocol error with message and code.
    #[error("JSON-RPC error: {0} (code {1})")]
    JsonRpc(String, i32),
    /// MCP initialization handshake failed.
    #[error("initialization failed: {0}")]
    Initialization(String),
    /// Serialization or deserialization error.
    #[error("serialization error: {0}")]
    Serialization(String),
}

impl From<serde_json::Error> for McpError {
    fn from(err: serde_json::Error) -> Self {
        McpError::Serialization(err.to_string())
    }
}

/// MCP adapter responsible for establishing a connection.
///
/// # Example
/// ```no_run
/// use pawbun_toolkit::mcp::{McpAdapter, TransportConfig};
///
/// let config = TransportConfig::Stdio {
///     command: "npx".into(),
///     args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into(), "/tmp".into()],
/// };
/// let mut session = McpAdapter::connect(config).unwrap();
/// let tools = session.list_tools().unwrap();
/// ```
#[derive(Debug)]
pub struct McpAdapter;

impl McpAdapter {
    /// Connects to an MCP server using the given transport configuration.
    ///
    /// Performs the `initialize` handshake and sends the `initialized` notification.
    pub fn connect(config: TransportConfig) -> Result<McpSession, McpError> {
        let mut transport: Box<dyn Transport> = match config {
            TransportConfig::Stdio { command, args } => {
                Box::new(StdioTransport::new(&command, &args)?)
            }
            #[cfg(feature = "http")]
            TransportConfig::Sse { url } => {
                Box::new(super::transport::SseTransport::new(url)?)
            }
            #[cfg(not(feature = "http"))]
            TransportConfig::Sse { .. } => {
                return Err(McpError::Transport(TransportError::Http(
                    "SSE requires the 'http' feature".into(),
                )));
            }
        };

        // Initialize handshake
        let init_req = JsonRpcRequest::new(
            0i64,
            "initialize",
            Some(json!(InitializeParams {
                protocol_version: "2024-11-05".into(),
                capabilities: json!({}),
                client_info: ClientInfo {
                    name: "pawbun-toolkit".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
            })),
        );

        let resp = transport.request(init_req)?;
        if let Some(err) = resp.error {
            return Err(McpError::Initialization(err.message));
        }

        // Send initialized notification
        let notify = JsonRpcRequest::notification("notifications/initialized", None);
        transport.request(notify)?; // Notification may return empty response

        Ok(McpSession {
            transport: Arc::new(Mutex::new(transport)),
            next_id: AtomicI64::new(1),
        })
    }
}

/// An active MCP session.
///
/// Holds the transport connection and provides methods to list and call MCP tools.
pub struct McpSession {
    transport: Arc<Mutex<Box<dyn Transport>>>,
    next_id: AtomicI64,
}

impl std::fmt::Debug for McpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpSession").finish_non_exhaustive()
    }
}

impl McpSession {
    /// Lists tools exposed by the MCP server.
    pub fn list_tools(&mut self) -> Result<Vec<McpToolDesc>, McpError> {
        let req = JsonRpcRequest::new(self.next_id.load(Ordering::SeqCst), "tools/list", None);
        self.next_id.fetch_add(1, Ordering::SeqCst);

        let resp = self
            .transport
            .lock()
            .map_err(|e| {
                McpError::Transport(TransportError::Io {
                    message: format!("mutex poisoned: {e}"),
                    kind: std::io::ErrorKind::Other,
                })
            })?
            .request(req)
            .map_err(McpError::Transport)?;

        let result = Self::unwrap_result(resp)?;
        let list: ListToolsResult = serde_json::from_value(result)?;
        Ok(list.tools)
    }

    /// Calls an MCP tool by name with the given arguments.
    pub fn call_tool(
        &mut self,
        name: impl Into<String>,
        arguments: Option<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let req = JsonRpcRequest::new(
            self.next_id.load(Ordering::SeqCst),
            "tools/call",
            Some(json!(CallToolParams {
                name: name.into(),
                arguments,
            })),
        );
        self.next_id.fetch_add(1, Ordering::SeqCst);

        let resp = self
            .transport
            .lock()
            .map_err(|e| {
                McpError::Transport(TransportError::Io {
                    message: format!("mutex poisoned: {e}"),
                    kind: std::io::ErrorKind::Other,
                })
            })?
            .request(req)
            .map_err(McpError::Transport)?;

        let result = Self::unwrap_result(resp)?;
        let call_result: CallToolResult = serde_json::from_value(result)?;
        Ok(call_result)
    }

    /// Closes the MCP session, terminating the transport connection.
    pub fn close(self) -> Result<(), McpError> {
        // Need to unwrap Arc<Mutex<Box<dyn Transport>>> to get ownership.
        // This is safe because we have ownership of self and no clones exist.
        let transport = Arc::into_inner(self.transport)
            .ok_or_else(|| {
                McpError::Transport(TransportError::Io {
                    message: "cannot close session: shared references exist".into(),
                    kind: std::io::ErrorKind::Other,
                })
            })?
            .into_inner()
            .map_err(|e| McpError::Transport(TransportError::Io {
                message: format!("mutex poisoned: {e}"),
                kind: std::io::ErrorKind::Other,
            }))?;
        transport.close().map_err(McpError::Transport)
    }

    fn unwrap_result(resp: JsonRpcResponse) -> Result<serde_json::Value, McpError> {
        if let Some(err) = resp.error {
            Err(McpError::JsonRpc(err.message, err.code))
        } else {
            Ok(resp.result.unwrap_or(serde_json::Value::Null))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::*;
    use crate::mcp::{JsonRpcId, JsonRpcRequest, JsonRpcResponse, Transport, TransportError};

    struct MockTransport {
        responses: Mutex<VecDeque<JsonRpcResponse>>,
    }

    impl MockTransport {
        fn new(responses: Vec<JsonRpcResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
            }
        }
    }

    impl std::fmt::Debug for MockTransport {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MockTransport").finish()
        }
    }

    impl Transport for MockTransport {
        fn request(&mut self, _req: JsonRpcRequest) -> Result<JsonRpcResponse, TransportError> {
            let mut guard = self.responses.lock().unwrap();
            guard.pop_front().ok_or(TransportError::UnexpectedEof)
        }

        fn close(self: Box<Self>) -> Result<(), TransportError> {
            Ok(())
        }
    }

    fn _build_init_response(id: impl Into<JsonRpcId>) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(id.into()),
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": { "name": "mock", "version": "1.0.0" }
            })),
            error: None,
        }
    }

    fn build_tools_list_response(id: impl Into<JsonRpcId>) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(id.into()),
            result: Some(json!({
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echo back the input",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "message": { "type": "string", "description": "Message to echo" }
                            },
                            "required": ["message"]
                        }
                    }
                ]
            })),
            error: None,
        }
    }

    fn build_tool_call_response(
        id: impl Into<JsonRpcId>,
        text: impl Into<String>,
    ) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(id.into()),
            result: Some(json!({
                "content": [{ "type": "text", "text": text.into() }],
                "isError": false
            })),
            error: None,
        }
    }

    fn mock_session(responses: Vec<JsonRpcResponse>) -> McpSession {
        McpSession {
            transport: Arc::new(Mutex::new(
                Box::new(MockTransport::new(responses)) as Box<dyn Transport>
            )),
            next_id: AtomicI64::new(0),
        }
    }

    #[test]
    fn test_mock_list_tools() {
        let mut session = mock_session(vec![build_tools_list_response(0i64)]);

        let tools = session.list_tools().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[0].description, Some("Echo back the input".into()));
    }

    #[test]
    fn test_mock_call_tool() {
        let mut session = mock_session(vec![build_tool_call_response(0i64, "hello world")]);

        let result = session
            .call_tool("echo", Some(json!({"message": "hello"})))
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        assert!(
            matches!(&result.content[0], crate::mcp::ToolContent::Text { text } if text == "hello world")
        );
    }

    #[cfg(feature = "http")]
    #[test]
    fn test_adapter_connect_sse_fails_on_real_url() {
        // Without a real SSE server the handshake will time out.
        let result = McpAdapter::connect(TransportConfig::Sse {
            url: "http://example.com/sse".into(),
        });
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("timed out") || err.contains("connection"));
    }
}
