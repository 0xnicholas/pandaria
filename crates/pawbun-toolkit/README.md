# pawbun-toolkit

Agent 工具抽象层。提供 `Tool` trait、`ToolKit` registry、同步/异步工具执行以及 MCP client adapter。

## 职责

- 定义 `Tool` trait — 每个工具的名称、描述、参数 schema 和执行逻辑
- 提供 `ToolKit` registry — 工具的注册、发现与调用
- 支持同步/异步工具执行（`ToolExecutor` / `AsyncToolExecutor`）
- 提供 `AsyncTool` trait 用于原生异步工具
- MCP client adapter — 连接外部 MCP server 作为工具
- 内置工具集（文件读写、代码执行、目录列表、视觉分析）

## 核心抽象

| Trait / 类型 | 说明 |
|---|---|
| `Tool` | 工具基础 trait — `name()`、`description()`、`parameters()`、`execute()` |
| `AsyncTool` | 异步工具 trait — `async_execute()` |
| `ToolKit` | 工具注册中心（HashMap，按名称查找） |
| `ToolRegistry` | 工具发现 trait — `list_tools()`、`get_tool()` |
| `ToolExecutor` | 同步工具执行 trait — `execute(name, args)` |
| `AsyncToolExecutor` | 异步工具执行 trait — `execute_async(name, args)` |
| `BlockingExecutor` | 可插拔的阻塞执行策略（用于 async 上下文） |
| `ToolResult` | 统一返回类型 — `content`、`details`、`is_error` |
| `ToolError` | 统一错误类型 |
| `ToolParameter` | 工具参数定义（兼容 JSON Schema） |

## 公开接口

| 模块 | 核心导出 |
|---|---|
| `tool` | `Tool` trait |
| `toolkit` | `ToolKit` |
| `async_tool` | `AsyncTool` trait、`BlockingExecutor` |
| `registry` | `ToolRegistry`、`ToolExecutor`、`AsyncToolExecutor` |
| `error` | `ToolError` |
| `types` | `ToolParameter`、`ToolResult` |
| `mcp` | MCP client adapter |
| `tools` | 内置工具：`FileReadTool`、`FileWriteTool`、`CodeExecuteTool`、`DirectoryListTool`、`VisionTool` |

## 快速使用

```rust
use pawbun_toolkit::{ToolKit, ToolExecutor, FileReadTool};

let mut toolkit = ToolKit::new();
toolkit.register(Box::new(FileReadTool::default()));

let result = toolkit.execute("file_read", r#"{"path": "README.md"}"#).unwrap();
println!("{}", result.content);
```

## 特性开关

| Feature | 说明 |
|---|---|
| `http` | HTTP 请求工具（`reqwest`） |
| `tokio` | 异步工具执行 |
| `schemars` | JSON Schema 生成 |
| `macros` | `#[pawbun_tool]` 过程宏 |
| `jsonpath` | JSONPath 查询支持 |
| `csv` | CSV 解析工具 |
| `tracing` | tracing span 注入 |
| `full` | 启用所有特性 |

## 依赖

- `serde` / `serde_json` — 序列化
- `thiserror` — 错误类型
- `async-trait` — async trait 支持
- `pawbun-toolkit-macros`（可选）— 过程宏
- `reqwest`（可选）— HTTP 工具
- `tokio`（可选）— 异步执行
