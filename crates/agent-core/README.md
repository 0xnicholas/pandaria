# agent-core

Agent loop 核心运行时。驱动 LLM tool use 协议的双层循环，管理 session 生命周期，定义 HookDispatcher 和 SessionStore 依赖反转边界。

## 职责

- 实现 ADR-001 定义的 agent loop：`UserMessage → AssistantMessage { ToolCall[] } → ToolResultMessage → ... → stop`
- 双层循环：外层 follow-up 消息队列、内层 turn tool 执行循环（支持串行/并行）
- 跨 provider 消息标准化：`llm_client::transform_messages()` 在每次 LLM 调用前自动处理 image downgrade、thinking block、tool call ID normalization、orphan padding
- `AgentTool` trait：工具抽象（名称、描述、参数 schema、执行模式、执行）
- `ToolExecutor`：工具执行管道（prepare → on_tool_call → execute → on_tool_result → finalize）
- `HookDispatcher` trait：hook 分发抽象（阻断型 + 链式 + 观测型），由调用方注入（常用 `DefaultHookDispatcher`）
- `SessionActor`：session 状态管理、prompt/steer/followUp/abort/shutdown 生命周期、Drop 清理
- `SessionStore` trait：持久化抽象，由 storage crate 实现

## 模块结构

```
src/
├── harness/          # 核心运行时（AgentLoop、SessionActor、ToolExecutor、CompactionActor）
├── hook/             # Hook 协议（HookDispatcher trait、DefaultHookDispatcher、CombinedDispatcher、*Ctx、*Mutation、超时保护）
├── memory/           # Memory 系统（MemoryStore trait、MemoryHookDispatcher、Conversation Formatter、EmeraldMemoryStore）
├── persistence/      # 持久化边界（SessionStore trait、SessionEntry）
├── prompt/           # Prompt 构建（PromptBuilder、PromptMutation）
├── skills/           # Skill 扫描、加载、注入
├── circuit_breaker.rs # LLM provider 调用熔断器
├── space.rs          # AgentSpace 统一目录抽象
├── utils/            # 工具函数与选项（sanitize、provider options）
├── types.rs          # 基础类型（AgentMessage、AgentTool trait 等）
├── error.rs          # 错误类型（AgentError、CompactionError）
├── events.rs         # 事件系统（AgentEvent）
├── file_ops.rs       # 文件操作提取器
└── test_utils.rs     # 测试辅助
```

## 公开接口

| 模块 | 子模块 | 核心导出 |
|---|---|---|
| `harness` | `agent_loop` | `AgentLoop`、`AgentLoopConfig`、`TurnResult` |
| `harness` | `session` | `SessionActor` |
| `harness` | `tool` | `ToolExecutor` |
| `harness` | `compaction` | `CompactionActor`、`CompactionConfig`、`should_compact` |
| `harness` | `error_recovery` | `RecoveryAction`、`RecoveryStateMachine` |
| `hook` | `dispatcher` | `HookDispatcher` trait |
| `hook` | `default_dispatcher` | `DefaultHookDispatcher` |
| `hook` | `combined` | `CombinedDispatcher` |
| `hook` | `context` | `ToolCallCtx`、`ToolResultCtx`、`TurnEndCtx`、`AgentEndCtx`、`SessionCtx`、`ContextCtx` |
| `hook` | `mutations` | `HookDecision`、`ToolResultMutation`、`ContextMutation`、`ToolCallMutation` |
| `hook` | `timeout` | `with_timeout` |
| `circuit_breaker` | — | `CircuitBreaker` |
| `memory` | `store` | `MemoryStore` trait |
| `memory` | `hook` | `MemoryHookDispatcher` |
| `memory` | `formatter` | Conversation Formatter |
| `memory` | `emerald` | `EmeraldMemoryStore`（HTTP adapter） |
| `persistence` | `store` | `SessionStore` trait |
| `persistence` | `entry` | `SessionEntry`、`CompactionDetails`、`SessionContextBuilder` |
| `prompt` | — | `PromptBuilder`、`PromptMutation` |
| `skills` | — | Skill 扫描、加载、注入 |
| `space` | — | `AgentSpace` |
| `utils` | `provider_opts` | `ProviderStreamOptions` |
| `utils` | `sanitize` | 敏感数据脱敏 |
| `types` | — | `AgentMessage`、`AgentTool` trait、`AgentToolResult`、`AgentToolRef`、`ToolExecutionMode` |
| `error` | — | `AgentError`、`CompactionError` |
| `events` | — | `AgentEvent`、`AgentEventListener` |
| `file_ops` | — | `FileOperationExtractor`、`DefaultFileOperationExtractor`、`FileOperations` |

## 边界

- **不知具体 HookDispatcher 实现**——通过 `HookDispatcher` trait 依赖反转，由 tenant 层或调用方注入
- **不知具体 LLM provider**——通过 `LlmProvider` trait 注入
- **不知具体持久化后端**——通过 `SessionStore` trait 注入，SessionActor 保存为 fire-and-forget，调用方可通过 `flush()` 确保持久化
- **tenant_id / session_id 贯穿所有操作**——所有 tracing span 和 context 结构体携带租户和会话标识
- **Hook 调用为直接函数调用**——无 Actor overhead；panic 由 `AgentLoop`/`ToolExecutor` 统一捕获，不传播到其他 session

## 依赖

- `ai-provider` — 消息类型、LlmProvider trait
- `pawbun-toolkit` — 工具抽象（Tool trait、ToolKit registry）
- `tokio` — 异步运行时
- `async-trait` — async trait 支持
- `thiserror` — 错误类型
- `serde_json` — JSON 类型（工具参数、details 字段）
- `tracing` — 事件记录，所有 span 携带 `tenant_id` / `session_id`
- `futures` — 并行工具执行（`join_all`）
- `tokio-util` — `CancellationToken`
