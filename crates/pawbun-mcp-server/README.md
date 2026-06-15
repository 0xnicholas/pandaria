# pawbun-mcp-server

MCP Server，将 Pawbun 工具暴露为 Model Context Protocol (MCP) 服务。

## 职责

- 注册 `ToolKit` → MCP 工具列表自动暴露
- 注册 `FileLoader` → MCP resources 自动暴露
- 支持 stdio（同步阻塞）和 SSE（axum HTTP）两种 transport
- MCP 初始化状态机（`initialize` → `initialized` → 正常请求处理）

## 公开接口

| 模块 | 核心导出 |
|---|---|
| `server` | `McpServer`、`McpServerBuilder` |
| `handler` | MCP 请求处理器（初始化 + 工具调用路由） |
| `transport` | Transport 实现（`StdioTransport`、`SseTransport`） |
| `capabilities` | 服务端能力声明（`tools`、`resources`） |
| `tool_bridge` | 内部工具桥接（toolkit → MCP handler） |
| `error` | `McpServerError` |

## 快速使用

### Stdio Server

```rust
use pawbun_mcp_server::McpServer;
use pawbun_toolkit::mcp::ServerTransportConfig;
use pawbun_toolkit::{ToolKit, FileReadTool};

let mut toolkit = ToolKit::new();
toolkit.register(Box::new(FileReadTool::default()));

let server = McpServer::builder("pawbun", "0.1.0")
    .register_toolkit(toolkit)
    .build();

server.launch(ServerTransportConfig::Stdio).unwrap();
```

### SSE Server (feature `http`)

```rust
use pawbun_mcp_server::McpServer;
use pawbun_toolkit::mcp::ServerTransportConfig;

let server = McpServer::builder("pawbun", "0.1.0")
    .register_toolkit(toolkit)
    .register_file_loader(loader)
    .build();

server.launch(ServerTransportConfig::Sse {
    host: "127.0.0.1".into(),
    port: 3000,
}).unwrap();
```

## 特性开关

| Feature | 说明 | 额外依赖 |
|---|---|---|
| `http` | SSE transport + axum HTTP server | `tokio`、`axum`、`tower-http`、`uuid`、`async-stream`、`futures` |

## 依赖

- `pawbun-toolkit` — 工具注册与执行
- `pawbun-files` — 文件加载
- `serde` / `serde_json` — JSON-RPC 序列化
- `thiserror` — 错误类型
- `axum`（可选）— HTTP transport
- `tokio`（可选）— 异步运行时
