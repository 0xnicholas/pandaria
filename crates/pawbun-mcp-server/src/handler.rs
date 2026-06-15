use pawbun_toolkit::mcp::*;
use pawbun_toolkit::{ToolExecutor, ToolKit, ToolRegistry};
use serde_json::Value;

/// Build an MCP input_schema JSON Schema object from toolkit ToolParameter list.
fn build_input_schema(params: &[pawbun_toolkit::ToolParameter]) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<Value> = Vec::new();

    for p in params {
        properties.insert(p.name.clone(), p.schema.clone());
        if p.required {
            required.push(Value::String(p.name.clone()));
        }
    }

    let mut schema = serde_json::Map::new();
    schema.insert("type".into(), Value::String("object".into()));
    if !properties.is_empty() {
        schema.insert("properties".into(), Value::Object(properties));
    }
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }

    Value::Object(schema)
}

/// MCP request handler with initialization state machine.
///
/// Lifecycle:
/// 1. Uninitialized — only `initialize` and `notifications/initialized` accepted.
/// 2. Initialized — `tools/list` and `tools/call` become available.
#[doc(hidden)]
pub struct RequestHandler {
    toolkit: ToolKit,
    server_info: ServerInfo,
    capabilities: Value,
    protocol_version: String,
    /// TODO: wire request_timeout wrapping into handle() for SSE/stdio transports.
    #[allow(dead_code)]
    request_timeout_ms: Option<u64>,
    initialized: bool,
}

impl RequestHandler {
    pub fn new(
        toolkit: ToolKit,
        server_info: ServerInfo,
        capabilities: Value,
        protocol_version: String,
        request_timeout_ms: Option<u64>,
    ) -> Self {
        Self {
            toolkit,
            server_info,
            capabilities,
            protocol_version,
            request_timeout_ms,
            initialized: false,
        }
    }

    /// Handle a single JSON-RPC request and produce a response.
    pub fn handle(&mut self, req: JsonRpcRequest) -> JsonRpcResponse {
        // Guard: reject non-handshake requests before initialization.
        if !self.initialized
            && !matches!(
                req.method.as_str(),
                "initialize" | "notifications/initialized"
            )
        {
            return JsonRpcResponse::error(req.id, -32002, "Server not initialized");
        }

        match req.method.as_str() {
            "initialize" => self.handle_initialize(req),
            "notifications/initialized" => self.handle_initialized(),
            "tools/list" => self.handle_list_tools(req),
            "tools/call" => self.handle_call_tool(req),
            _ => JsonRpcResponse::error(
                req.id,
                -32601,
                format!("Method not found: {}", req.method),
            ),
        }
    }

    fn handle_initialize(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let params: InitializeParams = match serde_json::from_value(req.params.unwrap_or_default())
        {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(req.id, -32602, format!("Invalid params: {e}"))
            }
        };

        if params.protocol_version != self.protocol_version {
            return JsonRpcResponse::error(
                req.id,
                -32603,
                format!(
                    "Unsupported protocol version: {}",
                    params.protocol_version
                ),
            );
        }

        JsonRpcResponse::ok_result(
            req.id,
            InitializeResult {
                protocol_version: self.protocol_version.clone(),
                capabilities: self.capabilities.clone(),
                server_info: self.server_info.clone(),
            },
        )
    }

    fn handle_initialized(&mut self) -> JsonRpcResponse {
        self.initialized = true;
        // MCP spec: notification does not expect a response.
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: None,
            result: None,
            error: None,
        }
    }

    fn handle_list_tools(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let tools: Vec<McpToolDesc> = self
            .toolkit
            .list()
            .into_iter()
            .map(|tool| McpToolDesc {
                name: tool.name().to_string(),
                description: Some(tool.description().to_string()),
                input_schema: Some(build_input_schema(&tool.parameters())),
            })
            .collect();

        JsonRpcResponse::ok_result(req.id, ListToolsResult { tools })
    }

    fn handle_call_tool(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let params: CallToolParams = match serde_json::from_value(req.params.unwrap_or_default())
        {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(req.id, -32602, format!("Invalid params: {e}"))
            }
        };

        let input_str = params
            .arguments
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_default();

        match self.toolkit.execute(&params.name, &input_str) {
            Ok(result) => {
                let call_result = CallToolResult {
                    content: vec![ToolContent::Text {
                        text: result.content,
                    }],
                    is_error: !result.success,
                };
                JsonRpcResponse::ok_result(req.id, call_result)
            }
            Err(e) => {
                let (code, msg) = match &e {
                    pawbun_toolkit::ToolError::NotFound(_) => {
                        (-32602, format!("Tool not found: {e}"))
                    }
                    _ => (-32603, e.to_string()),
                };
                JsonRpcResponse::error(req.id, code, msg)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawbun_toolkit::{Tool, ToolError, ToolParameter, ToolResult};
    use serde_json::json;
    use std::borrow::Cow;

    #[derive(Debug)]
    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes input."
        }
        fn parameters(&self) -> Cow<'static, [ToolParameter]> {
            Cow::Owned(vec![ToolParameter {
                name: "msg".into(),
                description: "Message".into(),
                required: true,
                schema: json!({"type": "string"}),
            }])
        }
        fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                success: true,
                content: input.to_string(),
                metadata: None,
                elapsed_ms: None,
            })
        }
    }

    fn make_handler() -> RequestHandler {
        let mut toolkit = ToolKit::new();
        toolkit.register(Box::new(EchoTool));
        RequestHandler::new(
            toolkit,
            ServerInfo {
                name: "test".into(),
                version: "0.1.0".into(),
            },
            json!({"tools": {}}),
            "2024-11-05".into(),
            None,
        )
    }

    fn do_initialize(handler: &mut RequestHandler) {
        let req = JsonRpcRequest::new(
            1i64,
            "initialize",
            Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1.0" }
            })),
        );
        let resp = handler.handle(req);
        assert!(resp.error.is_none(), "initialize should succeed");
        let req2 = JsonRpcRequest::notification("notifications/initialized", None);
        handler.handle(req2);
    }

    // ── initialize ──

    #[test]
    fn test_initialize_success() {
        let mut h = make_handler();
        let req = JsonRpcRequest::new(
            1i64,
            "initialize",
            Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1.0" }
            })),
        );
        let resp = h.handle(req);
        assert!(resp.error.is_none());
        let result: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(result.protocol_version, "2024-11-05");
        assert_eq!(result.server_info.name, "test");
    }

    #[test]
    fn test_initialize_wrong_version() {
        let mut h = make_handler();
        let req = JsonRpcRequest::new(
            1i64,
            "initialize",
            Some(json!({
                "protocolVersion": "2023-01-01",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1.0" }
            })),
        );
        let resp = h.handle(req);
        assert!(resp.is_error());
        assert_eq!(resp.error.unwrap().code, -32603);
    }

    // ── initialized notification ──

    #[test]
    fn test_initialized_sets_flag() {
        let mut h = make_handler();
        assert!(!h.initialized);
        let req = JsonRpcRequest::notification("notifications/initialized", None);
        let resp = h.handle(req);
        assert!(h.initialized);
        // notification should produce empty response
        assert!(resp.id.is_none());
        assert!(resp.result.is_none());
        assert!(resp.error.is_none());
    }

    // ── pre-initialization guard ──

    #[test]
    fn test_reject_tools_list_before_initialize() {
        let mut h = make_handler();
        let req = JsonRpcRequest::new(1i64, "tools/list", None);
        let resp = h.handle(req);
        assert!(resp.is_error());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32002);
        assert!(err.message.to_lowercase().contains("not initialized"));
    }

    #[test]
    fn test_reject_tools_call_before_initialize() {
        let mut h = make_handler();
        let req = JsonRpcRequest::new(
            1i64,
            "tools/call",
            Some(json!({
                "name": "echo",
                "arguments": {"msg": "hi"}
            })),
        );
        let resp = h.handle(req);
        assert!(resp.is_error());
        assert_eq!(resp.error.unwrap().code, -32002);
    }

    // ── tools/list ──

    #[test]
    fn test_list_tools_after_initialize() {
        let mut h = make_handler();
        do_initialize(&mut h);

        let req = JsonRpcRequest::new(2i64, "tools/list", None);
        let resp = h.handle(req);
        assert!(resp.error.is_none());
        let result: ListToolsResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "echo");
        assert!(result.tools[0].input_schema.is_some());
    }

    // ── tools/call ──

    #[test]
    fn test_call_tool_success() {
        let mut h = make_handler();
        do_initialize(&mut h);

        let req = JsonRpcRequest::new(
            3i64,
            "tools/call",
            Some(json!({
                "name": "echo",
                "arguments": {"msg": "hello"}
            })),
        );
        let resp = h.handle(req);
        assert!(resp.error.is_none());
        let result: CallToolResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        if let ToolContent::Text { text } = &result.content[0] {
            assert!(text.contains("hello"));
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_call_tool_not_found() {
        let mut h = make_handler();
        do_initialize(&mut h);

        let req = JsonRpcRequest::new(
            4i64,
            "tools/call",
            Some(json!({
                "name": "nonexistent",
                "arguments": {}
            })),
        );
        let resp = h.handle(req);
        assert!(resp.is_error());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32602);
        assert!(err.message.to_lowercase().contains("not found"));
    }

    #[test]
    fn test_unknown_method_before_init() {
        let mut h = make_handler();
        let req = JsonRpcRequest::new(5i64, "resources/list", None);
        let resp = h.handle(req);
        // Before init, everything non-handshake gets -32002
        assert_eq!(resp.error.unwrap().code, -32002);
    }

    #[test]
    fn test_unknown_method_after_init() {
        let mut h = make_handler();
        do_initialize(&mut h);

        let req = JsonRpcRequest::new(6i64, "resources/list", None);
        let resp = h.handle(req);
        // After init, unknown methods get -32601
        assert_eq!(resp.error.unwrap().code, -32601);
    }
}
