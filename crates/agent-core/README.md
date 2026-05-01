# agent-core

Agent loop 核心运行时。驱动 LLM tool use 协议的双层循环，管理 session 生命周期，定义 HookDispatcher 依赖反转边界。

## 职责

- 实现 ADR-001 定义的 agent loop：`UserMessage → AssistantMessage { ToolCall[] } → ToolResultMessage → ... → stop`
- 双层循环：外层 follow-up 消息队列、内层 turn tool 执行循环
- `AgentTool` trait：工具抽象（名称、描述、参数 schema、执行）
- `ToolExecutor`：工具执行管道（prepare → on_tool_call → execute → on_tool_result → finalize）
- `HookDispatcher` trait：hook 分发抽象（阻断型 + 链式 + 观测型），由 extensions crate 实现
- `SessionActor`：session 状态管理、prompt/steer/followUp/abort 生命周期

## 公开接口

| 模块 | 核心导出 |
|---|---|
| `types` | `AgentMessage`、`AgentTool` trait、`AgentToolResult`、`AgentToolRef`、`ToolExecutionMode` |
| `context` | `ToolCallCtx`、`ToolResultCtx`、`TurnEndCtx`、`AgentEndCtx`、`SessionCtx`、`ContextCtx` |
| `mutations` | `HookDecision`（Continue / Block）、`ToolResultMutation`、`ContextMutation` |
| `hook_dispatcher` | `HookDispatcher` trait（阻断型 `on_tool_call`、链式 `on_tool_result`/`on_context`、观测型 `on_turn_end`/`on_agent_end`/`on_session_start`） |
| `tool` | `ToolExecutor` |
| `loop_` | `AgentLoop` |
| `session` | `SessionActor` |
| `error` | `AgentError` |

## 边界

- **不知 Extension 的存在**——通过 `HookDispatcher` trait 依赖反转，extensions crate 实现此 trait
- **不知具体 LLM provider**——通过 `LlmProvider` trait 注入
- **不管理租户或持久化**——sesion 生命周期管理在本层，但租户调度和持久化在上层（tenant、persistence）

## 依赖

- `llm-client` — 消息类型、LlmProvider trait
- `tokio` — 异步运行时
- `async-trait` — async trait 支持
- `thiserror` — 错误类型
- `serde` / `serde_json` — JSON 类型
- `tracing` — 事件记录
- `futures` / `tokio-util` — Stream 和 CancellationToken
