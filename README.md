# Pandaria

面向服务端多租户的 agent runtime & harness，以 Rust 实现，提供进程级会话隔离、资源配额、可观测性，以及基于 Actor Mailbox + EventBus 的混合 Extension 系统。

---

## 产品说明

### 解决的问题

现有 AI agent 工具（如 pi.dev、Claude Code）以**单用户单进程**为设计前提，直接在终端中运行一个 agent loop。但在服务端场景下，这种方式存在根本性缺陷：

| 问题 | 说明 |
|---|---|
| 无租户隔离 | 多个用户共享同一进程空间，一个用户的 agent 可能泄漏信息给另一个用户 |
| 无资源调度 | 无法按租户分配 CPU 时间片、限制并发 session 数或计量 token 消耗 |
| 无可观测性 | 缺少 per-tenant 的 tracing、metrics、错误率统计 |
| 无持久化 | session 状态仅存在内存中，进程重启后所有上下文丢失 |
| 扩展不安全 | 第三方插件直接运行在主进程内，panic 会传播到 agent loop |

Pandaria 从架构层面解决这些问题，为构建**多租户 AI agent 平台**提供稳固的运行时基础。

### 核心能力

| 能力 | 实现方式 | 状态 |
|---|---|---|
| 多租户会话隔离 | 每 session 独立的 tokio task 树，零共享可变状态 | 🟡 开发中 |
| 资源配额 | 每租户 CPU time budget、并发上限、token 消耗计量 | 🔲 计划中 |
| 可观测性 | 基于 `tracing`，所有 span 带 `tenant_id` / `session_id` | 🔲 计划中 |
| Session 持久化 | 消息历史与 compaction 结果存 Redis / PostgreSQL | 🔲 计划中 |
| 安全 Extension 系统 | Actor 隔离 + panic 捕获 + 超时保护，panic 不传播 | 🟡 开发中 |
| 多 Provider 支持 | Anthropic、OpenAI、Google、Mistral（AwsBedrock feature gate） | 🟡 开发中 |
| 水平扩展 | Session 持久化 + 跨节点迁移 | 🔲 计划中 |

### 与现有工具对比

| 维度 | pi.dev / Claude Code | Pandaria |
|---|---|---|
| 运行模式 | 单用户 CLI 进程 | 多租户服务端 runtime |
| 会话隔离 | 进程级（天然隔离，但不可控） | tokio task 级（可控、可观测） |
| 资源配额 | 无 | 每租户 CPU / 并发 / token |
| Extension 安全 | 进程内（panic 可传播） | Actor 隔离 + 超时保护 |
| 持久化 | 本地文件（非结构化） | 外部存储（Redis / PG） |
| 可观测性 | 无 | tracing span + metrics |

---

## 技术架构

### 系统总览

```
┌──────────────────────────────────────────────────────────┐
│                        Pandaria                           │
├──────────────────────────────────────────────────────────┤
│                                                            │
│  ┌─────────────┐     HTTP / SSE                           │
│  │    tui      │────────────┐                             │
│  │ (独立二进制)  │            │                            │
│  └─────────────┘            │                             │
│                              ▼                             │
│  ┌──────────────────────────────────┐  🔲 计划中          │
│  │          api-gateway             │                     │
│  │   gRPC / WebSocket  认证  限流   │                     │
│  └──────────────┬───────────────────┘                     │
│                  │                                         │
│  ┌───────────────┴────────────────────┐  🔲 计划中        │
│  │             tenant                  │                   │
│  │   调度器  配额管理  Session 注册表   │                   │
│  └───────────────┬────────────────────┘                   │
│                  │                                         │
│  ┌───────────────┴────────────────────┐                    │
│  │           extensions               │                    │
│  │  Extension trait  HookRouter       │                    │
│  │  ExtensionActor   EventBus         │                    │
│  │  Manager  Audit  RateLimit  ToolGuard│                  │
│  └───────────────┬────────────────────┘                    │
│                  │                                         │
│  ┌───────────────┴────────────────────┐                    │
│  │           agent-core               │                    │
│  │  SessionActor   AgentLoop          │                    │
│  │  ToolExecutor   CompactionActor    │                    │
│  │  HookDispatcher RecoveryStateMachine│                   │
│  └───────────────┬────────────────────┘                    │
│                  │                                         │
│  ┌───────────────┴────────────────────┐                    │
│  │           ai-provider               │                    │
│  │  Anthropic  OpenAI  Google  Mistral │                    │
│  │  SSE  重试  校验  兼容  修复        │                    │
│  └────────────────────────────────────┘                    │
│                                                            │
│  ┌────────────┐  ┌──────────────┐  🔲 计划中              │
│  │  storage   │ │ observability│                          │
│  │Redis/PG     │ │tracing/metrics│                         │
│  └────────────┘  └──────────────┘                          │
│                                                            │
└──────────────────────────────────────────────────────────┘

依赖方向:  tui → api-gateway → tenant → extensions → agent-core → ai-provider
                                            ↓              ↓
                                       storage        observability
```

### Agent Loop 协议

基于 LLM 原生 tool calling 协议，每个 turn 对应一次 LLM 响应：

```
                    ┌──────────────────────────────┐
                    │        SessionActor           │
                    │  系统提示词 + 历史消息          │
                    └──────────────┬───────────────┘
                                   │ session.entries
                                   ▼
                    ┌──────────────────────────────┐
                    │    SessionContextBuilder      │
                    │  构建 LLM 可见上下文           │
                    └──────────────┬───────────────┘
                                   │ LlmContext { messages, tools }
                                   ▼
                    ┌──────────────────────────────┐
                    │        AgentLoop::run()       │
                    │                               │
                    │  ┌─────────────────────────┐  │
                    │  │ on_before_provider_request│  │
                    │  └────────────┬────────────┘  │
                    │               ▼               │
                    │  ┌─────────────────────────┐  │
                    │  │  LlmProvider::stream()   │  │
                    │  │  SSE → MessageEventStream │  │
                    │  └────────────┬────────────┘  │
                    │               ▼               │
                    │  ┌─────────────────────────┐  │
                    │  │ on_after_provider_response│  │
                    │  └────────────┬────────────┘  │
                    │               │               │
                    │         stop_reason?          │
                    │        ╱    │    ╲            │
                    │   Stop   ToolUse  其他        │
                    │    │        │                 │
                    │    │   ┌────┴────┐            │
                    │    │   │ToolExecutor│          │
                    │    │   │ ┌───────┐ │          │
                    │    │   │ │并行执行│ │          │
                    │    │   │ │多工具  │ │          │
                    │    │   │ └───────┘ │          │
                    │    │   └────┬─────┘            │
                    │    │        │ ToolResultMessage │
                    │    │        │ 插入历史          │
                    │    │        ▼                  │
                    │    │   下一轮 loop ──────┐      │
                    │    │                     │      │
                    │    ▼                     │      │
                    │  on_agent_end            │      │
                    │  session.save() ◄────────┘      │
                    └──────────────────────────────────┘
```

### Session 隔离架构

每个租户 session 是独立的 tokio task 树，跨 Session 通信通过 `tokio::mpsc`：

```
┌─────────────────────────────────────────────────────┐
│ Tenant A                                            │
│                                                     │
│  ┌─ SessionActor (tenant_a, session_1) ────────────┐│
│  │                                                  ││
│  │  ┌─────────────────────────────────────┐         ││
│  │  │         AgentLoopActor              │         ││
│  │  │  驱动 tool use loop                 │         ││
│  │  │  消费 AssistantMessageEventStream    │         ││
│  │  └──────────────┬──────────────────────┘         ││
│  │                 │                                 ││
│  │  ┌──────────────┴──────────────────────┐         ││
│  │  │       ToolExecutorActor              │         ││
│  │  │  并行/顺序执行工具调用                │         ││
│  │  │  拦截 hook → execute → 结果 hook     │         ││
│  │  └──────────────┬──────────────────────┘         ││
│  │                 │                                 ││
│  │  ┌──────────────┴──────────────────────┐         ││
│  │  │     ExtensionActor[]                 │         ││
│  │  │  per-extension mailbox               │         ││
│  │  │  oneshot reply（拦截）               │         ││
│  │  │  broadcast（观测）                   │         ││
│  │  └──────────────┬──────────────────────┘         ││
│  │                 │                                 ││
│  │  ┌──────────────┴──────────────────────┐         ││
│  │  │       CompactionActor               │         ││
│  │  │  上下文压缩（结构化摘要生成）          │         ││
│  │  └─────────────────────────────────────┘         ││
│  │                                                  ││
│  │  mpsc ─────────────────────────────────────────  ││
│  └──────────────────────────────────────────────────┘│
│                                                     │
│  ┌─ SessionActor (tenant_a, session_2) ────────────┐│
│  │  ...独立的 AgentLoopActor / ToolExecutor / etc.  ││
│  └──────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────┐
│ Tenant B                                            │
│  ┌─ SessionActor (tenant_b, session_3) ────────────┐│
│  │  ...完全独立的 task 树，零共享状态               ││
│  └──────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────┘
```

### Hook 路由机制

两种传输通道按消息语义路由：

```
                  ExtensionManager / HookRouter
                           │
          ┌────────────────┼────────────────┐
          │                │                │
          ▼                ▼                ▼
   阻断型拦截 hook     链式拦截 hook      观测型 hook
   on_tool_call       on_tool_result    on_turn_end
   on_before_compact  on_context        on_agent_end
                      on_before_*       on_session_start
                      on_after_*        ...
          │                │                │
          ▼                ▼                ▼
   Actor Mailbox      Actor Mailbox      EventBus
   + oneshot          + oneshot          broadcast
          │                │                │
          ▼                ▼                ▼
   ┌─────────────┐  ┌─────────────────┐  ┌───────────────┐
   │ 串行调用     │  │ 串行调用         │  │ 并发广播       │
   │ 等待回复     │  │ 每个 handler     │  │ 各自 spawn    │
   │             │  │ 在前一个结果     │  │ fire & forget │
   │ first-block  │  │ 上叠加修改       │  │              │
   │ -wins       │  │ (chain merge)    │  │ 100ms 超时    │
   │             │  │                 │  │ 静默丢弃       │
   │ 500ms 超时  │  │ 500ms 超时      │  │              │
   │ 默认 Continue │  │ 跳过该 handler  │  │              │
   └──────┬──────┘  └────────┬────────┘  └──────────────┘
          │                  │
   首个 Block          所有 Continue
   即停止              → 累积后的
   并返回              最终 mutation
```

### Extension 生命周期

```
ExtensionManager::spawn_all()
│
├─ Extension::tools() → 收集工具定义
│                       (first-registration-wins)
│
├─ 为每个 Extension 创建 ExtensionActor
│  └─ tokio::spawn(actor.run())
│     ├─ 主循环: tokio::select!
│     │  ├─ mpsc::recv (Mailbox) → 执行 hook
│     │  │  └─ tokio::spawn(hook)
│     │  │     └─ 拦截型 chain: 500ms 超时
│     │  └─ broadcast::recv (EventBus) → 观测型
│     │     └─ tokio::spawn(observed_hook)
│     │        └─ 100ms 超时
│     │
│     └─ 收到 Shutdown → break
│
├─ 创建 HookRouter
│  └─ 持有所有 ExtensionHandle
│  └─ 实现 HookDispatcher trait
│     ├─ on_tool_call() → 串行 ask, first-block-wins
│     ├─ on_tool_result() → 串行 ask, chain-merge
│     ├─ on_turn_end() → broadcast emit
│     └─ ...
│
├─ 创建 ExtensionTool (AgentTool wrapper)
│  └─ execute() → ExtensionHandle::execute_tool()
│
└─ 返回 (HookRouter, Vec<AgentToolRef>)

ExtensionManager::shutdown_all()
│
├─ 发送 Shutdown 到每个 ExtensionActor
├─ 等待所有 actor task join
└─ 释放所有 Handle
```

### 端到端数据流

```
 ┌──────┐     ┌──────────┐     ┌──────────┐     ┌────────────┐
 │ User │────▶│  tui /   │────▶│ agent-core│────▶│ ai-provider │
 │      │     │ gateway  │     │           │     │            │
 └──────┘     └──────────┘     └──────────┘     └────────────┘

 1. 用户输入 ──▶ SessionActor::prompt("帮我重构这个文件")

 2. SessionActor::run_with_messages()
    │
    ├─ SessionContextBuilder::build_context()
    │  └─ SessionEntry[] → LlmContext { system_prompt, messages, tools }
    │
    ├─ HookRouter::on_before_agent_start() → BeforeAgentStartMutation
    │  └─ 逐 Extension 链式合并（可修改 system_prompt、messages）
    │
    ├─ AgentLoop::run()
    │  │
    │  ├─ [Turn Loop]
    │  │  │
    │  │  ├─ drain steer_queue → 注入消息
    │  │  │
    │  │  ├─ HookRouter::on_context() → ContextMutation
    │  │  │  └─ 解析孤立的 tool call → 填充占位结果
    │  │  │
    │  │  ├─ HookRouter::on_before_provider_request() → ProviderRequestMutation
    │  │  │  └─ 可修改 system_prompt、messages、tools、stream_options
    │  │  │
    │  │  ├─ LlmProvider::stream(model, context, options, CancellationToken)
    │  │  │  │
    │  │  │  ├─ 指数退避重试（RateLimited / Overloaded / Timeout）
    │  │  │  │
    │  │  │  ├─ SSE 事件流 → AssistantMessageEventStream
    │  │  │  │  └─ Start → TextStart → TextDelta → TextEnd →
    │  │  │  │     ThinkingStart → ThinkingDelta → ThinkingEnd →
    │  │  │  │     ToolCallStart → ToolCallDelta → ToolCallEnd →
    │  │  │  │     Done { reason, message }
    │  │  │  │
    │  │  │  └─ 返回 AssistantMessage
    │  │  │
    │  │  ├─ HookRouter::on_after_provider_response() → ProviderResponseMutation
    │  │  │  └─ 可修改 content、stop_reason
    │  │  │
    │  │  ├─ 若 stop_reason = ToolUse → 工具执行
    │  │  │  │
    │  │  │  ├─ 遍历 tool_calls（并行或顺序，取决于 ToolExecutionMode）
    │  │  │  │
    │  │  │  └─ ToolExecutor::execute_tool()
    │  │  │     │
    │  │  │     ├─ HookRouter::on_tool_call() → (HookDecision, ToolCallMutation)
    │  │  │     │  └─ 若 Block → 跳过该工具，生成错误结果
    │  │  │     │
    │  │  │     ├─ 工具执行（AgentTool::execute）
    │  │  │     │  └─ 扩展工具 → ExtensionHandle::execute_tool()（无超时）
    │  │  │     │  └─ panic 捕获 → AgentError
    │  │  │     │
    │  │  │     └─ HookRouter::on_tool_result() → ToolResultMutation
    │  │  │        └─ 链式合并（可修改 content、details、is_error、terminate）
    │  │  │
    │  │  ├─ 将 ToolResultMessage 插入 session.entries
    │  │  │
    │  │  └─ 若 all_terminate → 提前退出 loop
    │  │  │
    │  │  └─ HookRouter::on_turn_end() → broadcast (fire-and-forget)
    │  │
    │  ├─ drain follow_up_queue → 若还有消息，继续新 turn
    │  │
    │  └─ Loop 终止条件: stop_reason = Stop | Error | Aborted
    │
    ├─ HookRouter::on_agent_end() → broadcast
    │
    ├─ CompactionActor::compact_if_needed()
    │  └─ 超过 token 阈值 → HookRouter::on_before_compact()
    │     → LLM 生成结构化摘要 → 替换历史 → on_compact_end() broadcast
    │
    └─ 若 RecoveryStateMachine 评估为需要恢复:
       ├─ overflow → compact-and-retry
       ├─ retryable → backoff retry
       └─ exhausted → abort
```

---

## Crate 清单

### ai-provider — LLM Provider 抽象层

纯通信层，不负责租户上下文、session 生命周期或资源配额。

| 模块 | 功能 |
|---|---|
| `providers/` | Anthropic、OpenAI、Google、Mistral 四个 Provider 实现（AwsBedrock 在 feature gate 下） |
| `models.rs` | `ModelRegistry` — 47+ 模型注册表，支持按 model name 查找 provider 和 tokens per dollar |
| `compat.rs` | 跨 provider 兼容层 — `OpenAiCompat`（20+ 标志位）、`AnthropicCompat`、自动检测与合并 |
| `validation.rs` | JSON Schema 校验工具调用参数，支持类型强制转换 |
| `overflow.rs` | 上下文溢出检测 — 19 种 regex 模式匹配各 provider 的溢出错误 |
| `repair.rs` | 流式 JSON 修复 — `StreamingJsonParser` 渐进解析 + 启发式修复 |
| `retry.rs` | 指数退避重试 — 100ms 基础延迟，最多 3 次，RateLimited / Overloaded / Timeout 可重试 |
| `transform.rs` | 跨 provider 消息转换 — 图片降级、thinking block 移除、tool call ID 归一化 |
| `hooks.rs` | 请求/响应钩子 — `OnPayloadFn` / `OnResponseFn` |
| `types.rs` | 核心类型 — `Content`、`Message`、`ToolDef`、`Usage`、`StopReason`、`LlmError` 等 |

关键 trait：

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn provider_name(&self) -> &str;
    fn models(&self) -> Vec<String>;
    fn api_for(&self, model: &str) -> Api;
    async fn stream(&self, model: &str, context: LlmContext,
                    options: StreamOptions, signal: CancellationToken)
        -> Result<AssistantMessageEventStream, LlmError>;
}
```

### agent-core — Agent Loop 运行时

Agent loop 引擎，管理 session 生命周期、工具执行、hook 调度和上下文压缩。

| 模块 | 功能 |
|---|---|
| `agent_loop.rs` | `AgentLoop` — 驱动 tool use loop，消费 SSE 事件流，协调 hook 和工具执行 |
| `session_actor.rs` | `SessionActor` — per-tenant session 生命周期管理，消息历史、steer/follow-up 队列、持久化边界 |
| `tool_executor.rs` | `ToolExecutor` — 完整工具执行管线：`on_tool_call` → `execute()` → `on_tool_result` |
| `compaction_actor.rs` | `CompactionActor` — 自动压缩上下文，生成结构化摘要 |
| `hook_dispatcher.rs` | `HookDispatcher` trait — 19 个 hook 方法的依赖反转边界 |
| `recovery.rs` | `RecoveryStateMachine` — 错误恢复状态机，overflow / retryable / abort 判断 |
| `context.rs` | 所有 hook 上下文类型（`ToolCallCtx`、`TurnEndCtx`、`SessionCtx` 等） |
| `mutations.rs` | 所有 hook 返回的 mutation 类型 |
| `events.rs` | `AgentEvent` 枚举及 `AgentEventListener` trait |
| `session_entry.rs` | `SessionEntry`、`SessionContextBuilder` — 消息历史构建 |
| `agent_tool.rs` | `AgentTool` trait — 工具注册接口 |
| `storage.rs` | `SessionStore` trait — 持久化抽象边界 |

关键 trait：

```rust
#[async_trait]
pub trait HookDispatcher: Send + Sync {
    // 阻断型（first-block-wins）
    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation);
    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision;
    // 链式（chain merge）
    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation;
    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation;
    // 观测型（broadcast, fire-and-forget）
    async fn on_turn_end(&self, _ctx: &TurnEndCtx);
    async fn on_agent_end(&self, _ctx: &AgentEndCtx);
    // ...
}
```

### extensions — Extension 系统

Extension trait 定义、Actor Mailbox / EventBus 基础设施和内置 extension。

| 模块 | 功能 |
|---|---|
| `host/extension.rs` | `Extension` trait — 19 个 hook 方法 + `tools()` + `execute_tool()` |
| `host/extension_actor.rs` | `ExtensionActor` — 每个 Extension 的 Actor 运行时，`ExtensionHandle` — 外部调用句柄 |
| `host/event_bus.rs` | `EventBus<T>` — 泛型广播通道，100ms 超时 |
| `host/hook_router.rs` | `HookRouter` — 实现 `HookDispatcher`，将 hook 路由到 ExtensionActor |
| `host/manager.rs` | `ExtensionManager` — 完整生命周期管理，spawn / shutdown / 工具收集 |
| `host/extension_tool.rs` | `ExtensionTool` — 将 Extension 注册的工具包装为 `AgentTool` |
| `builtins/audit.rs` | `AuditExtension` — 观测型，记录所有 tool call 和 turn 到 tracing journal |
| `builtins/rate_limit.rs` | `RateLimitExtension` — 滑动窗口工具调用频率限制 |
| `builtins/tool_guard.rs` | `ToolGuardExtension` — allowlist / denylist 访问控制 |

关键 trait：

```rust
#[async_trait]
pub trait Extension: Send + Sync {
    fn name(&self) -> &str;
    fn tools(&self) -> Vec<ToolDef>;
    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation);
    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation;
    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation;
    async fn execute_tool(&self, tool_call_id: &str, params: serde_json::Value)
        -> Result<AgentToolResult, AgentError>;
    // ...
}
```

### tui — 终端 UI 客户端

独立二进制 `pandaria-tui`，基于 ratatui + crossterm，通过 REST + SSE 与服务端通信。

| 功能 | 说明 |
|---|---|
| 多 Session 管理 | 创建、切换、查看 session 列表 |
| Markdown 渲染 | pulldown-cmark 渲染，syntect 语法高亮 |
| 流式通信 | SSE 事件流实时更新消息内容 |
| 自动补全 | 文件路径和 slash 命令补全 |
| 剪贴板 | 粘贴支持 |
| 配置 | TOML 配置文件 + CLI 参数覆盖 |

---

## 快速开始

### 构建

```bash
cargo build --release
```

### 运行 TUI 客户端

```bash
cargo run -p pandaria-tui
```

### 运行测试

```bash
cargo test
```

### 作为库使用

```rust
use pandaria_llm_client::{LlmProvider, AnthropicProvider, LlmContext, StreamOptions};
use tokio_util::sync::CancellationToken;
use pandaria_agent_core::{SessionActor, SessionStore, HookDispatcher};
use pandaria_extensions::{ExtensionManager, AuditExtension};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. 创建 LLM provider
    let provider = AnthropicProvider::new("claude-sonnet-4-5-20250929");
    let model = provider.models()[0].clone();

    // 2. 创建 extension 管理器
    let mut manager = ExtensionManager::new();
    manager.register(Arc::new(AuditExtension::new()));

    // 3. spawn extensions → 得到 HookDispatcher + tools
    let (hook_dispatcher, tools) = manager.spawn_all().await;

    // 4. 创建 session
    let session = SessionActor::new(
        tenant_id: "tenant_1".into(),
        session_id: "session_1".into(),
        hook_dispatcher: Arc::new(hook_dispatcher),
        provider: Arc::new(provider),
        model,
        tools,
        system_prompt: "你是一个专业的软件工程师。".into(),
        ..Default::default()
    );

    // 5. 发送 prompt
    session.prompt("帮我重构这个文件".into()).await;

    // 6. 关闭
    manager.shutdown_all().await;
    Ok(())
}
```

---

## 开发路线图

- [ ] `api-gateway` crate — gRPC / WebSocket 接入层
- [ ] `tenant` crate — 租户调度器、配额管理
- [ ] `storage` crate — Redis / PostgreSQL SessionStore 实现
- [ ] `observability` crate — tracing 集成、per-tenant metrics
- [ ] AWS Bedrock provider（feature gate `bedrock` 已有代码，待集成验证）
- [ ] Session 持久化 schema 设计
- [ ] WASM / RPC Extension 边界实现

---

## 参考资料

- [AGENTS.md](./AGENTS.md) — 项目上下文文档，包含完整 ADR 记录、模块边界、关键约束
- [pi.dev](https://pi.dev) — Agent loop 语义的参考实现

---

## 许可证

[MIT](LICENSE)
