//! JSON-RPC 2.0 and MCP protocol message types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON-RPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// JSON-RPC version (always "2.0").
    pub jsonrpc: String,
    /// Request ID (`None` for notifications).
    pub id: Option<JsonRpcId>,
    /// Method name to invoke.
    pub method: String,
    /// Method parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A JSON-RPC response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// JSON-RPC version (always "2.0").
    pub jsonrpc: String,
    /// Response ID (matches request ID).
    pub id: Option<JsonRpcId>,
    /// Result value (present on success).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error object (present on failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code.
    pub code: i32,
    /// Error message.
    pub message: String,
    /// Additional error data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC ID can be a number, string, or null.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum JsonRpcId {
    /// Numeric ID.
    Number(i64),
    /// String ID.
    String(String),
    /// Null ID (used in notifications).
    Null,
}

impl JsonRpcRequest {
    /// Creates a new JSON-RPC request.
    pub fn new(id: impl Into<JsonRpcId>, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: Some(id.into()),
            method: method.into(),
            params,
        }
    }

    /// Creates a notification (no id).
    pub fn notification(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: None,
            method: method.into(),
            params,
        }
    }
}

impl JsonRpcResponse {
    /// Checks if the response contains an error.
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }

    /// Returns the result or an error.
    pub fn into_result(self) -> Result<Value, JsonRpcError> {
        if let Some(err) = self.error {
            Err(err)
        } else {
            Ok(self.result.unwrap_or(Value::Null))
        }
    }
}

impl From<i64> for JsonRpcId {
    fn from(v: i64) -> Self {
        JsonRpcId::Number(v)
    }
}

impl From<String> for JsonRpcId {
    fn from(v: String) -> Self {
        JsonRpcId::String(v)
    }
}

impl From<&str> for JsonRpcId {
    fn from(v: &str) -> Self {
        JsonRpcId::String(v.into())
    }
}

// -----------------------------------------------------------------------------
// MCP-specific types
// -----------------------------------------------------------------------------

/// MCP initialize request params.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Protocol version string (e.g. "2024-11-05").
    pub protocol_version: String,
    /// Client capabilities.
    pub capabilities: Value,
    /// Client identification info.
    pub client_info: ClientInfo,
}

/// MCP client info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    /// Client name.
    pub name: String,
    /// Client version.
    pub version: String,
}

/// MCP initialize result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Protocol version string.
    pub protocol_version: String,
    /// Server capabilities.
    pub capabilities: Value,
    /// Server identification info.
    pub server_info: ServerInfo,
}

/// MCP server info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// Server name.
    pub name: String,
    /// Server version.
    pub version: String,
}

/// A tool description returned by `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDesc {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: Option<String>,
    /// JSON Schema for tool input parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
}

/// Result of `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResult {
    /// Available tools.
    pub tools: Vec<McpToolDesc>,
}

/// Params for `tools/call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolParams {
    /// Tool name to call.
    pub name: String,
    /// Tool arguments.
    pub arguments: Option<Value>,
}

/// Result of `tools/call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    /// Result content items.
    pub content: Vec<ToolContent>,
    /// Whether the tool call resulted in an error.
    #[serde(default)]
    pub is_error: bool,
}

/// A single content item in a tool call result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolContent {
    /// Text content.
    #[serde(rename = "text")]
    Text {
        /// Text value.
        text: String,
    },
    /// Image content (Base64-encoded).
    #[serde(rename = "image")]
    Image {
        /// Base64-encoded image data.
        data: String,
        /// Image MIME type.
        mime_type: String,
    },
    /// Resource reference.
    #[serde(rename = "resource")]
    Resource {
        /// Resource value.
        resource: Value,
    },
}

// -----------------------------------------------------------------------------
// JsonRpcResponse convenience constructors
// -----------------------------------------------------------------------------

impl JsonRpcResponse {
    /// Construct a successful response with a raw Value result.
    pub fn ok(id: Option<JsonRpcId>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Construct a successful response, serializing the result.
    pub fn ok_result(id: Option<JsonRpcId>, result: impl Serialize) -> Self {
        let value = serde_json::to_value(result).unwrap_or(Value::Null);
        Self::ok(id, value)
    }

    /// Construct an error response with a standard JSON-RPC error code.
    pub fn error(id: Option<JsonRpcId>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_response_ok() {
        let resp = JsonRpcResponse::ok(Some(JsonRpcId::Number(1)), json!("result"));
        assert_eq!(resp.id, Some(JsonRpcId::Number(1)));
        assert_eq!(resp.result, Some(json!("result")));
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_response_ok_result_serializes() {
        let resp =
            JsonRpcResponse::ok_result(Some(JsonRpcId::String("x".into())), "hello");
        assert_eq!(resp.result, Some(json!("hello")));
    }

    #[test]
    fn test_response_error() {
        let resp = JsonRpcResponse::error(
            Some(JsonRpcId::Number(42)),
            -32601,
            "Method not found",
        );
        assert!(resp.is_error());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn test_response_ok_no_id() {
        let resp = JsonRpcResponse::ok(None, json!(42));
        assert!(resp.id.is_none());
        assert_eq!(resp.result, Some(json!(42)));
    }
}
