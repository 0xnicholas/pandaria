# Pandaria

面向服务端多租户的 agent runtime & harness，以 Rust 实现，提供进程级会话隔离、资源配额、可观测性，以及内联的 hook 策略系统。

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
| 多租户会话隔离 | 每 session 独立的 tokio task 树，零共享可变状态 | ✅ 已完成 |
| 资源配额 | 每租户并发上限、token 消耗计量（滑动窗口）；CPU time budget 预留字段 | 🟡 部分完成 |
| 可观测性 | 基于 `tracing`，所有 span 带 `tenant_id` / `session_id` | ✅ 基础已完成 |
| Session 持久化 | 消息历史与 compaction 结果存 Redis / PostgreSQL | ✅ 已完成 |
| Hook 策略系统 | 内联 `DefaultHookDispatcher`，直接函数调用，panic 由调用方统一捕获 | ✅ 已完成 |
| 多 Provider 支持 | Anthropic、OpenAI、Google、Mistral、DeepSeek（AWS Bedrock feature-gated 占位） | ✅ 已完成 |
| 水平扩展 | Session 持久化 + 跨节点迁移 | 🔲 远期规划 |

### 与现有工具对比

| 维度 | pi.dev / Claude Code | Pandaria |
|---|---|---|
| 运行模式 | 单用户 CLI 进程 | 多租户服务端 runtime |
| 会话隔离 | 进程级（天然隔离，但不可控） | tokio task 级（可控、可观测） |
| 资源配额 | 无 | 每租户并发 / token（CPU time 待实现） |
| Hook 策略安全 | 进程内（panic 可传播） | 直接函数调用 + 调用方统一捕获 panic |
| 持久化 | 本地文件（非结构化） | 外部存储（Redis / PG） |
| 可观测性 | 无 | tracing span + per-tenant 标识 |

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
│  ┌──────────────────────────────────┐  ✅ 已完成          │
│  │          api-gateway             │                     │
│  │   REST + SSE  认证  限流         │                     │
│  └──────────────┬───────────────────┘                     │
│                  │                                         │
│  ┌───────────────┴────────────────────┐  ✅ 已完成        │
│  │             tenant                  │                   │
│  │   调度器  配额管理  Session 注册表   │                   │
│  └───────────────┬────────────────────┘                   │
│                  │                                         │
│  ┌───────────────┴────────────────────┐                    │
│  │           agent-core               │                    │
│  │  SessionActor   AgentLoop          │                    │
│  │  ToolExecutor   CompactionActor    │                    │
│  │  HookDispatcher DefaultHookDispatcher│                  │
│  └───────────────┬────────────────────┘                    │
│                  │                                         │
│  ┌───────────────┴────────────────────┐                    │
│  │           ai-provider               │                    │
│  │  Anthropic  OpenAI  Google  Mistral │                    │
│  │  SSE  重试  校验  兼容  修复        │                    │
│  └────────────────────────────────────┘                    │
│                                                            │
│  ┌────────────┐  ┌──────────────┐  ✅ 已完成              │
│  │  storage   │ │ sanitize     │                          │
│  │Redis/PG     │ │(agent-core)  │                         │
│  └────────────┘  └──────────────┘                         │
│                                                            │
└──────────────────────────────────────────────────────────┘

依赖方向:  tui → api-gateway → tenant → agent-core → ai-provider
                   ↓
              storage
```

> **注意**：v0.1.x 移除了 `extensions` crate。原内置策略（audit、path_guard、tool_guard、token_budget）已内联至 `agent-core/src/hook/default_dispatcher.rs` 中的 `DefaultHookDispatcher`。Hook 调用为直接函数调用，无 Actor、无 EventBus。若未来需要第三方插件，将重新设计更轻量的插件运行时。

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
│  │  │         AgentLoop                   │         ││
│  │  │  驱动 tool use loop（同步调用）       │         ││
│  │  │  消费 AssistantMessageEventStream    │         ││
│  │  └──────────────┬──────────────────────┘         ││
│  │                 │                                 ││
│  │  ┌──────────────┴──────────────────────┐         ││
│  │  │       ToolExecutor                   │         ││
│  │  │  并行/顺序执行工具调用                │         ││
│  │  │  hook → execute → 结果 hook          │         ││
│  │  └──────────────┬──────────────────────┘         ││
│  │                 │                                 ││
│  │  ┌──────────────┴──────────────────────┐         ││
│  │  │       DefaultHookDispatcher          │         ││
│  │  │  内联 hook 策略（直接函数调用）        │         ││
│  │  │  无 Actor、无 EventBus                │         ││
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
│  │  ...独立的 AgentLoop / ToolExecutor / etc.       ││
│  └──────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────┐
│ Tenant B                                            │
│  ┌─ SessionActor (tenant_b, session_3) ────────────┐│
│  │  ...完全独立的 task 树，零共享状态               ││
│  └──────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────┘
```

### Hook 调用机制

`DefaultHookDispatcher` 内联所有 hook 策略，直接函数调用：

| Hook 类型 | 调用方式 | 合并策略 |
|---|---|---|
| 阻断型拦截（`on_tool_call`、`on_before_compact`） | 直接函数调用 | first-block-wins |
| 链式拦截（`on_tool_result`、`on_context`、`on_before_agent_start`、`on_before_provider_request`、`on_after_provider_response`） | 直接函数调用 | 链式合并 |
| 观测型（`on_turn_end`、`on_agent_end`、`on_session_start` 等） | 直接函数调用 | 无需合并 |

**优势：**
- 零 Actor overhead（无 mpsc、无 oneshot）
- panic 行为直接暴露（由 `AgentLoop`/`ToolExecutor` 统一捕获）
- 代码路径清晰，便于调试

**代价：**
- 失去 Extension 级 panic 隔离（单个 session 内 panic 会影响该 session）
- 失去第三方动态扩展能力（未来需重新设计 WASM / RPC 插件边界）

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
    ├─ DefaultHookDispatcher::on_before_agent_start() → BeforeAgentStartMutation
    │  └─ 链式合并（可修改 system_prompt、messages）
    │
    ├─ AgentLoop::run()
    │  │
    │  ├─ [Turn Loop]
    │  │  │
    │  │  ├─ drain steer_queue → 注入消息
    │  │  │
    │  │  ├─ DefaultHookDispatcher::on_context() → ContextMutation
    │  │  │  └─ 解析孤立的 tool call → 填充占位结果
    │  │  │
    │  │  ├─ DefaultHookDispatcher::on_before_provider_request() → ProviderRequestMutation
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
    │  │  ├─ DefaultHookDispatcher::on_after_provider_response() → ProviderResponseMutation
    │  │  │  └─ 可修改 content、stop_reason
    │  │  │
    │  │  ├─ 若 stop_reason = ToolUse → 工具执行
    │  │  │  │
    │  │  │  ├─ 遍历 tool_calls（并行或顺序，取决于 ToolExecutionMode）
    │  │  │  │
    │  │  │  └─ ToolExecutor::execute_tool()
    │  │  │     │
    │  │  │     ├─ DefaultHookDispatcher::on_tool_call() → (HookDecision, ToolCallMutation)
    │  │  │     │  └─ 若 Block → 跳过该工具，生成错误结果
    │  │  │     │
    │  │  │     ├─ 工具执行（AgentTool::execute）
    │  │  │     │  └─ panic 捕获 → AgentError
    │  │  │     │
    │  │  │     └─ DefaultHookDispatcher::on_tool_result() → ToolResultMutation
    │  │  │        └─ 链式合并（可修改 content、details、is_error、terminate）
    │  │  │
    │  │  ├─ 将 ToolResultMessage 插入 session.entries
    │  │  │
    │  │  └─ 若 all_terminate → 提前退出 loop
    │  │  │
    │  │  └─ DefaultHookDispatcher::on_turn_end() → 观测型，直接调用
    │  │
    │  ├─ drain follow_up_queue → 若还有消息，继续新 turn
    │  │
    │  └─ Loop 终止条件: stop_reason = Stop | Error | Aborted
    │
    ├─ DefaultHookDispatcher::on_agent_end() → 观测型
    │
    ├─ CompactionActor::compact_if_needed()
    │  └─ 超过 token 阈值 → on_before_compact() → 摘要替换历史
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
| `harness/agent_loop.rs` | `AgentLoop` — 驱动 tool use loop，消费 SSE 事件流，协调 hook 和工具执行 |
| `harness/session.rs` | `SessionActor` — per-tenant session 生命周期管理，消息历史、steer/follow-up 队列、持久化边界 |
| `harness/tool.rs` | `ToolExecutor` — 完整工具执行管线：`on_tool_call` → `execute()` → `on_tool_result` |
| `harness/compaction.rs` | `CompactionActor` — 自动压缩上下文，生成结构化摘要 |
| `harness/error_recovery.rs` | `RecoveryStateMachine` — 错误恢复状态机，overflow / retryable / abort 判断 |
| `hook/dispatcher.rs` | `HookDispatcher` trait — 19 个 hook 方法的依赖反转边界 |
| `hook/default_dispatcher.rs` | `DefaultHookDispatcher` — 内联实现，整合 audit、path-guard、tool-guard、token-budget |
| `hook/context.rs` | 所有 hook 上下文类型（`ToolCallCtx`、`TurnEndCtx`、`SessionCtx` 等） |
| `hook/mutations.rs` | 所有 hook 返回的 mutation 类型 |
| `persistence/` | `SessionStore` trait — 持久化抽象边界；`SessionEntry`、`SessionContextBuilder` |
| `skills/` | Skill 扫描、加载、注入 |
| `space.rs` | `AgentSpace` — 统一目录抽象（`~/.pandaria/`） |
| `utils/sanitize.rs` | 敏感数据脱敏 |

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
    // 观测型（直接调用）
    async fn on_turn_end(&self, _ctx: &TurnEndCtx);
    async fn on_agent_end(&self, _ctx: &AgentEndCtx);
    // ...（共 19 个 hook 方法）
}
```

### tenant — 多租户控制平面

租户注册表、配额管理、session 跟踪和资源计量。

| 模块 | 功能 |
|---|---|
| `registry.rs` | `TenantRegistry` — 全局并发租户注册表 |
| `supervisor.rs` | `TenantSupervisor` — per-tenant session 跟踪器和配额执行器 |
| `quota.rs` | `TenantQuota` — 可配置限制（sessions、tokens、tool calls、CPU） |
| `manager.rs` | `TenantManager` trait / `TenantManagerImpl` — session 生命周期管理 |
| `extensions/` | `TenantQuotaExtension` — 工具调用频率限制；`TenantTokenMeterExtension` — token 计量 |
| `session_guard.rs` | `SessionGuard` — session 槽位 RAII 守卫 |

### api-gateway — HTTP 接入层

服务端 HTTP 入口，提供 REST API + SSE 事件流。

| 功能 | 说明 |
|---|---|
| 认证 | Bearer token HMAC-SHA256 |
| 路由 | REST API（session CRUD + message + SSE） |
| 限流 | per-tenant 令牌桶 |

### storage — 持久化层

通用存储适配器，实现 `agent_core::SessionStore` trait。

| 适配器 | 模块 | 状态 |
|---|---|---|
| `PgSessionStore` | `session::postgres` | 生产就绪 |
| `RedisSessionStore` | `session::redis` | 可用 |

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
cargo run -p tui
```

### 运行服务端

```bash
cargo run -p api-gateway
```

### 运行测试

```bash
# 单元测试（全部通过）
cargo test --workspace --lib

# 集成测试（PostgreSQL 依赖 Docker，或使用本地实例）
cargo test -p storage --test integration_postgres -- --test-threads=1
```

### 作为库使用

```rust
use ai_provider::{LlmProvider, AnthropicProvider, LlmContext, StreamOptions};
use tokio_util::sync::CancellationToken;
use agent_core::{SessionActor, SessionStore, HookDispatcher};
use agent_core::hook::DefaultHookDispatcher;
use agent_core::space::AgentSpace;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. 创建 LLM provider
    let provider = AnthropicProvider::new("claude-sonnet-4-5-20250929");
    let model = provider.models()[0].clone();

    // 2. 创建内联 hook dispatcher（内置策略）
    let space = AgentSpace::from_env_or_default();
    let hook_dispatcher = DefaultHookDispatcher::builder()
        .space(space.clone())
        .denied_tools(vec!["dangerous_command".into()])
        .path_guard_fields(vec!["path".into(), "file_path".into()])
        .build();

    // 3. 创建 session
    let session = SessionActor::new(
        tenant_id: "tenant_1".into(),
        session_id: "session_1".into(),
        hook_dispatcher: Arc::new(hook_dispatcher),
        provider: Arc::new(provider),
        model,
        tools: vec![],
        system_prompt: "你是一个专业的软件工程师。".into(),
        ..Default::default()
    );

    // 4. 发送 prompt
    session.prompt("帮我重构这个文件".into()).await;

    Ok(())
}
```

---

## 开发路线图

### v0.1.x（当前）

- [x] `ai-provider` crate — 5 个 Provider + SSE + 重试/校验/修复
- [x] `agent-core` crate — AgentLoop、SessionActor、ToolExecutor、CompactionActor、HookDispatcher、DefaultHookDispatcher、PromptBuilder、Skills 注入
- [x] `storage` crate — PostgreSQL + Redis SessionStore
- [x] `tenant` crate — 租户注册表、并发配额、token/tool call 计量
- [x] `api-gateway` crate — REST + SSE + HMAC 认证 + 限流
- [x] `tui` crate — 完整终端客户端
- [x] AgentSpace 统一目录（`~/.pandaria/`）
- [x] 理解型多模态（Image/Video/Audio 输入）
- [x] 生成型多模态（MediaProvider + MediaGenerationTool）

### v0.2.0（中期）

- [x] Session 持久化加固：auto-restore + 增量保存（`append_entries`）+ PG jsonb 串联优化
- [x] Memory 数据准备重构：Conversation Formatter + MemoryStore trait 简化 + MemoryHookDispatcher 重写
- [x] E2E 测试矩阵扩展：5 个新测试（持久化恢复、compaction、故障注入、并发隔离、MemoryStore 联动）
- [ ] CPU time 预算实现与接入
- [ ] Bedrock provider 接入 Router/Resolver
- [ ] compaction 大文本操作移至 `spawn_blocking`
- [ ] 水平扩展：session 跨节点迁移能力设计
- [ ] WASM / RPC 插件运行时（重新设计轻量级插件边界）

---

## 参考文档

- [AGENTS.md](./AGENTS.md) — 项目上下文文档，包含完整 ADR 记录、模块边界、关键约束
- [PRD.md](./PRD.md) — 产品需求文档

---

## 许可证

[MIT](LICENSE)
