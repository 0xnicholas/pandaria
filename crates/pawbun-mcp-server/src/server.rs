use pawbun_toolkit::mcp::{ServerInfo, ServerTransport, ServerTransportConfig};
use pawbun_toolkit::{Tool, ToolKit};
use pawbun_files::DefaultFileLoader;
use serde_json::{json, Value};

use crate::capabilities::{LogLevel, ServerCapabilities};
use crate::error::McpServerError;
use crate::handler::RequestHandler;

/// MCP Server exposing Pawbun tools via Model Context Protocol.
pub struct McpServer {
    toolkit: ToolKit,
    server_info: ServerInfo,
    capabilities: Value,
    protocol_version: String,
    cors_origins: Vec<String>,
    request_timeout_ms: Option<u64>,
}

/// Builder for [`McpServer`].
///
/// # Example
/// ```no_run
/// use pawbun_mcp_server::McpServer;
/// use pawbun_toolkit::mcp::ServerTransportConfig;
/// use pawbun_toolkit::{ToolKit, FileReadTool};
/// use pawbun_files::DefaultFileLoader;
///
/// let mut toolkit = ToolKit::new();
/// toolkit.register(Box::new(FileReadTool::default()));
///
/// let loader = DefaultFileLoader::with_base_dir("/app/data");
///
/// let server = McpServer::builder("pawbun", "0.1.0")
///     .register_toolkit(toolkit)
///     .register_file_loader(loader)
///     .build();
///
/// // Blocking stdio server
/// server.launch(ServerTransportConfig::Stdio).unwrap();
/// ```
pub struct McpServerBuilder {
    toolkit: ToolKit,
    file_loader: Option<DefaultFileLoader>,
    server_name: String,
    server_version: String,
    protocol_version: String,
    capabilities: ServerCapabilities,
    raw_capabilities: Option<Value>,
    cors_origins: Vec<String>,
    request_timeout_ms: Option<u64>,
    tool_timeout_ms: Option<u64>,
}

impl McpServerBuilder {
    /// Create a builder with server name and version.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            toolkit: ToolKit::new(),
            file_loader: None,
            server_name: name.into(),
            server_version: version.into(),
            protocol_version: "2024-11-05".into(),
            capabilities: ServerCapabilities::default(),
            raw_capabilities: None,
            cors_origins: Vec::new(),
            request_timeout_ms: Some(30_000),
            tool_timeout_ms: None,
        }
    }

    /// Register a ToolKit. Assigns directly (single-toolkit for now).
    pub fn register_toolkit(mut self, toolkit: ToolKit) -> Self {
        self.toolkit = toolkit;
        self
    }

    /// Register a FileLoader. Automatically wraps as `file_read` and `file_list` tools.
    ///
    /// **Deduplication**: if a tool with the same name already exists in the toolkit,
    /// the bridge tool is skipped — user-registered tools take priority.
    pub fn register_file_loader(mut self, loader: DefaultFileLoader) -> Self {
        self.file_loader = Some(loader);
        self
    }

    /// Register a single tool. Same-name tools are overwritten.
    pub fn register_tool(mut self, tool: Box<dyn Tool>) -> Self {
        self.toolkit.register(tool);
        self
    }

    /// Override default MCP protocol version (default: `"2024-11-05"`).
    pub fn protocol_version(mut self, version: impl Into<String>) -> Self {
        self.protocol_version = version.into();
        self
    }

    /// Enable tools capability.
    pub fn with_tools_capability(mut self) -> Self {
        self.capabilities.tools = Some(crate::capabilities::ToolsCapability { list_changed: false });
        self
    }

    /// Enable logging capability with the specified level.
    pub fn with_logging_capability(mut self, level: LogLevel) -> Self {
        self.capabilities.logging = Some(crate::capabilities::LoggingCapability { level });
        self
    }

    /// Enable prompts capability.
    pub fn with_prompts_capability(mut self) -> Self {
        self.capabilities.prompts = Some(crate::capabilities::PromptsCapability);
        self
    }

    /// Enable resources capability.
    pub fn with_resources_capability(mut self) -> Self {
        self.capabilities.resources = Some(crate::capabilities::ResourcesCapability);
        self
    }

    /// Set request timeout in milliseconds (default: 30_000).
    pub fn request_timeout(mut self, ms: u64) -> Self {
        self.request_timeout_ms = Some(ms);
        self
    }

    /// Set tool call timeout in milliseconds.
    /// If set, overrides the ToolKit's default timeout.
    pub fn tool_timeout(mut self, ms: u64) -> Self {
        self.tool_timeout_ms = Some(ms);
        self
    }

    /// Set allowed CORS origins for SSE transport.
    #[cfg(feature = "http")]
    pub fn cors_origins(mut self, origins: Vec<String>) -> Self {
        self.cors_origins = origins;
        self
    }

    /// Override default capabilities with a raw JSON value (backward compatible).
    pub fn capabilities(mut self, caps: Value) -> Self {
        self.raw_capabilities = Some(caps);
        self
    }

    /// Build the [`McpServer`], registering all bridge tools.
    pub fn build(mut self) -> McpServer {
        if let Some(loader) = self.file_loader.take() {
            crate::tool_bridge::register_bridge_tools(&mut self.toolkit, loader);
        }
        let caps = if let Some(raw) = self.raw_capabilities {
            raw
        } else {
            serde_json::to_value(&self.capabilities).unwrap_or_else(|_| json!({"tools": {}}))
        };
        if let Some(ms) = self.tool_timeout_ms {
            self.toolkit = ToolKit::with_timeout(ms);
        }
        McpServer {
            toolkit: self.toolkit,
            server_info: ServerInfo {
                name: self.server_name,
                version: self.server_version,
            },
            capabilities: caps,
            protocol_version: self.protocol_version,
            cors_origins: self.cors_origins,
            request_timeout_ms: self.request_timeout_ms,
        }
    }
}

impl McpServer {
    /// Create a builder.
    pub fn builder(name: impl Into<String>, version: impl Into<String>) -> McpServerBuilder {
        McpServerBuilder::new(name, version)
    }

    /// Start the server with the given transport configuration.
    /// Blocks the current thread until the transport closes.
    pub fn launch(self, config: ServerTransportConfig) -> Result<(), McpServerError> {
        match config {
            ServerTransportConfig::Stdio => {
                let transport =
                    Box::new(crate::transport::stdio::StdioServerTransport::new());
                self.run_loop(transport)
            }
            #[cfg(feature = "http")]
            ServerTransportConfig::Sse { bind_addr } => {
                let config = crate::transport::sse::SseServerConfig::new(&bind_addr)
                    .with_cors_origins(self.cors_origins.clone());
                let transport = crate::transport::sse::SseServerTransport::new_with_config(config)
                    .map_err(McpServerError::Bind)?;
                self.run_loop(Box::new(transport))
            }
            #[cfg(not(feature = "http"))]
            ServerTransportConfig::Sse { .. } => Err(McpServerError::Bind(
                "SSE transport requires the 'http' feature".into(),
            )),
        }
    }

    fn run_loop(
        self,
        mut transport: Box<dyn ServerTransport>,
    ) -> Result<(), McpServerError> {
        let mut handler = RequestHandler::new(
            self.toolkit,
            self.server_info,
            self.capabilities,
            self.protocol_version,
            self.request_timeout_ms,
        );

        loop {
            let req = match transport.recv() {
                Ok(req) => req,
                Err(pawbun_toolkit::mcp::TransportError::UnexpectedEof) => break,
                Err(e) => return Err(e.into()),
            };

            let resp = handler.handle(req);
            transport.send(resp)?;
        }

        transport.close()?;
        Ok(())
    }
}
