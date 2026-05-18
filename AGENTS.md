# AGENTS.md

> 本文件是项目的首要上下文文档。所有参与开发的 agent（包括 AI coding agent 和人类工程师）在开始任何任务前必须阅读本文件。

---

## 项目定义

**面向服务端多租户的 agent runtime & harness，以 Rust 实现，提供进程级会话隔离、资源配额、可观测性，以及基于 Actor Mailbox + EventBus 的混合 Extension 系统。**

本项目解决的核心问题：现有 agent 工具（如 pi.dev）以单用户单进程为设计前提，无法在服务端场景下为多个租户提供安全隔离、资源公平调度和生产级可观测性。本项目从架构层面重新设计，而非在现有工具上打补丁。

---

## 核心设计决策（ADR 摘要）

### ADR-001：Agent Loop 采用原生 Tool Use 协议

Agent loop 基于 LLM 原生 tool calling 协议：

```
UserMessage → AssistantMessage { ToolCall[] } → ToolResultMessage → ... → AssistantMessage { stop }
```

每个 turn 对应一次 LLM 响应，loop 在 `stop_reason = "stop"` 时终止。支持单个 AssistantMessage 内的并行 ToolCall（一次 LLM 响应多个工具调用）。

### ADR-002：Extension 系统采用 Rust trait，编译期注册

当前阶段 Extension 仅对内，以 Rust trait object 实现：

```rust
#[async_trait]
pub trait Extension: Send + Sync {
    fn name(&self) -> &str;
    fn tools(&self) -> Vec<ToolDef>;
    fn tool_execution_modes(&self) -> HashMap<String, ToolExecutionMode> {
        HashMap::new()
    }

    // 阻断型拦截 hook — first-block-wins
    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        (HookDecision::Continue, ToolCallMutation::default())
    }
    async fn on_before_compact(&self, ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Continue
    }

    // 链式拦截 hook — 链式合并
    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
        ToolResultMutation::default()
    }
    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        ContextMutation::default()
    }
    async fn on_before_agent_start(&self, ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
        BeforeAgentStartMutation::default()
    }
    async fn on_before_provider_request(&self, ctx: &ProviderRequestCtx) -> ProviderRequestMutation {
        ProviderRequestMutation::default()
    }
    async fn on_after_provider_response(&self, ctx: &ProviderResponseCtx) -> ProviderResponseMutation {
        ProviderResponseMutation::default()
    }

    // 工具执行委托（30s 框架超时）
    async fn execute_tool(&self, tool_call_id: &str, params: Value) -> Result<AgentToolResult, AgentError> {
        Err(AgentError::ToolExecutionFailed("not implemented".into()))
    }

    // 观测型 hook — fire-and-forget
    async fn on_turn_end(&self, ctx: &TurnEndCtx) {}
    async fn on_agent_end(&self, ctx: &AgentEndCtx) {}
    async fn on_session_start(&self, ctx: &SessionCtx) {}
    async fn on_tool_execution_start(&self, ctx: &ToolExecutionStartCtx) {}
    async fn on_tool_execution_end(&self, ctx: &ToolExecutionEndCtx) {}
    async fn on_compact_end(&self, ctx: &CompactEndCtx) {}
}
```

`Arc<dyn Extension>` 是未来扩展到 WASM / RPC extension 的抽象边界，设计时保留此缝隙，不做破坏性假设。

### ADR-003：Hook 机制采用 Actor Mailbox + EventBus 混合模型

两种通道按消息语义路由，而非按性能需求路由：

| Hook 类型 | 传输方式 | 执行模式 | 合并策略 |
|---|---|---|---|
| 阻断型拦截（`on_tool_call`、`on_before_compact`） | Actor Mailbox + `oneshot` | 串行，等待回复 | first-block-wins |
| 链式拦截（`on_tool_result`、`on_context`、`on_before_agent_start`、`on_before_provider_request`、`on_after_provider_response`） | Actor Mailbox + `oneshot` | 串行，等待回复 | 链式合并（每个 handler 在前一个结果的修改上叠加） |
| 观测型（`on_turn_end`、`on_agent_end`、`on_session_start`、`on_tool_execution_start`、`on_tool_execution_end`、`on_compact_end`） | EventBus broadcast | 并发，不等待 | 无需合并 |

**合并策略详解：**

- **阻断型合并**：遍历 Extension 列表，逐个串行调用。首个返回 `HookDecision::Block` 即停止并返回。所有返回 `Continue` 则放行。handler 可对 `ctx.input` 做 in-place 修改。
- **链式合并**：遍历 Extension 列表，逐个串行调用。每个 handler 返回 mutation struct（`ToolResultMutation { content, details, isError }` 或 `ContextMutation { messages }`），下一个 handler 接收已叠加 mutation 的 ctx。最终返回累积后的结果。
- **观测型合并**：通过 EventBus 并发广播给所有 Extension，各自 spawn 独立 task。fire-and-forget，不等待返回值，100ms 超时静默丢弃。

Extension 间通信规则：
- **点对点**（A 调用 B，期待回复）：Actor Mailbox ask pattern
- **广播通知**（A 通知多个 Extension）：EventBus emit
- **禁止**：在处理 Mailbox 消息的过程中发起新的同步 ask（防止死锁）

### ADR-004：Session 隔离采用 tokio task 级别

每个租户 session 是一个独立的 tokio task 树，Session 之间不共享任何可变状态：

```
TenantSupervisor
  └── SessionActor (tenant_id, session_id)
        ├── AgentLoop            # 驱动 tool use loop（SessionActor 内部同步调用）
        ├── ToolExecutor         # 并行工具执行（SessionActor 内部同步调用）
        ├── ExtensionActor[]     # 每个 Extension 独立 Actor（mpsc mailbox）
        ├── CompactionActor      # 上下文压缩（SessionActor 内部同步调用）
        └── EventProcessor       # 事件处理（tokio::mpsc）
```

所有跨 Actor 通信通过 `tokio::mpsc`，禁止共享 `Arc<Mutex<_>>` 作为跨 session 状态。

> **实现说明**：当前版本中 `AgentLoop`、`ToolExecutor` 和 `CompactionActor` 是 `SessionActor` 内部直接调用的组件，而非独立的 tokio actor（无 mpsc mailbox）。`ExtensionActor` 和 `EventProcessor` 遵循 Actor 模型（通过 mpsc 通信）。这一设计简化了 session 内部状态流转，同时保留了 Extension 系统的 Actor 隔离。未来如需将 AgentLoop/CompactionActor 独立为 actor，可在不破坏外部接口的前提下重构。

### ADR-005：多租户三个基础能力不可裁剪

1. **资源配额**：每租户 CPU time budget、并发 session 数上限、token 消耗计量。
2. **Session 持久化**：消息历史和 compaction 结果持久化到外部存储（Redis / PostgreSQL），服务重启后 session 可恢复，支持跨节点迁移。
3. **可观测性**：基于 `tokio-tracing`，所有 span 携带 `tenant_id` 和 `session_id`，支持 per-tenant tool call 耗时、token 消耗、错误率统计。

---

## 模块边界

```
crates/
  agent-core/        # Agent loop、Tool pipeline、Session 生命周期、Compaction、HookDispatcher trait
    harness/         #   核心运行时（AgentLoop、SessionActor、ToolExecutor、CompactionActor）
    hook/            #   Hook 协议（HookDispatcher trait、*Ctx、*Mutation、超时边界）
    persistence/     #   持久化边界（SessionStore trait、SessionEntry）
    utils/           #   工具与选项
  extensions/        # Extension 系统
    host/            #   Extension trait、Actor Mailbox、EventBus、Hook 路由
    builtins/        #   内置 Extension 实现（audit、rate-limit、tool-guard 等）
  tenant/            # Tenant Scheduler、配额管理、Session 注册表
  storage/          # 通用存储层（Session 状态序列化、Redis/PG 适配器）
  observability/     # tracing 集成、metrics、per-tenant 统计
  ai-provider/        # LLM provider 抽象、流式 SSE 解析、HTTP 通信协议
  api-gateway/       # REST + SSE 接入、认证、限流
  tui/               # 终端客户端（ratatui + REST client + SSE 订阅）
```

**ai-provider 边界说明：**

- **纯通信层**：不负责 tenant 上下文、session 生命周期、资源配额检查。这些由调用方（`agent-core` / `tenant` 层）通过 tracing span 注入。
- **HTTP 连接**：由 `reqwest::Client` 内部管理。支持上层通过 `with_client()` 注入统一配置的 Client 以复用连接，但连接池本身不由 ai-provider 维护。
- **可观测性**：ai-provider 内部不创建 tracing span。调用方（`agent-core`）应在调用 `stream()` 前创建带 `tenant_id`/`session_id` 的 span。
- **Token 计量**：ai-provider 返回 `Usage` 原始数据，per-tenant 计量由调用方（`agent-core` 或 `tenant` 层）计算。

**依赖方向严格单向**（禁止反向依赖）：

```
api-gateway → tenant → extensions → agent-core → ai-provider
                   ↓         ↓
              storage        observability
```

---

## 关键约束

### 并发模型

- 所有异步代码使用 `tokio`，禁止 `std::thread::sleep` 等阻塞调用出现在 async 上下文中。
- CPU 密集型操作（大文本压缩、序列化）使用 `tokio::task::spawn_blocking`。
- 阻断型和链式 hook 的 `oneshot` reply 必须设置超时（默认 500ms）。阻断型超时后默认 `HookDecision::Continue`；链式超时跳过该 handler，继续后续 handler。
- 观测型 hook 的 `tokio::spawn` 任务必须设置超时（默认 100ms），超时静默丢弃，不影响 agent loop。

### 错误处理

- 所有跨 crate 的错误类型使用 `thiserror` 定义。
- 禁止 `.unwrap()` 出现在非测试代码中，使用 `?` 或显式 `expect("reason")`。
- Extension panic 不得传播到 agent loop，通过 `tokio::task` JoinHandle 捕获并记录。
- LLM API 调用必须实现指数退避重试（最多 3 次），并在 tracing span 中记录重试次数。

### 安全约束

- `tenant_id` 必须在所有 tracing span 和日志中出现，禁止无 tenant 上下文的操作日志。
- Extension 访问文件系统时必须经过路径校验，禁止访问 `/workspace/{tenant_id}/` 以外的路径。
- LLM API Key 不得出现在任何日志、tracing span、错误消息或 panic 信息中。

### 代码规范

- 所有公开 API 必须有文档注释（`///`）。
- 新 crate 必须包含 `README.md`，描述该 crate 的职责、公开接口和边界。
- 集成测试放在 `tests/`，使用 `testcontainers` 启动 Redis/PG 依赖，禁止测试依赖外部网络。
  - **本地 PostgreSQL 备用**：`crates/storage/tests/integration_postgres.rs` 支持通过 `PANDARIA_TEST_PG_URL` 环境变量连接本地 PostgreSQL（无需 Docker）。使用本地 DB 时必须加 `--test-threads=1`，因为所有测试共享同一个数据库。示例：
    ```bash
    pg_ctl -D "$HOME/Library/Application Support/Postgres/var-18" start
    PANDARIA_TEST_PG_URL="postgres://postgres@localhost:5432/postgres" \
      cargo test -p storage --test integration_postgres -- --test-threads=1
    ```

---

## 参考系统对照

本项目的 agent loop 语义以 [pi.dev](./_references/pi-main) 为参考实现。

**pi 概念 → 本项目对应：**

| pi | 本项目 |
|---|---|
| `AgentSession` | `SessionActor` |
| `pi.on("tool_call")` | `Extension::on_tool_call` → Actor Mailbox |
| `pi.on("turn_end")` | `Extension::on_turn_end` → EventBus |
| `pi.registerTool()` | `Extension::tools()` 编译期注册 |
| `session.compact()` | `CompactionActor::compact()` |
| Extension npm 包 | 不支持（当前阶段仅内部 Rust crate） |
| `/reload` 热更新 | 不支持（服务端无此需求） |

**pi 不支持、本项目必须支持：**

- 租户隔离与资源配额
- Session 跨重启持久化与跨节点迁移
- 水平扩展
- 生产级可观测性（per-tenant metrics、distributed tracing）
- API 接入层（认证、限流、WebSocket / gRPC）

---

## 当前状态

| 项目 | 状态 |
|---|---|
| 技术栈（Rust + tokio） | ✅ 已确定 |
| Extension 模型（Rust trait） | ✅ 已确定 |
| Hook 机制（Mailbox + EventBus） | ✅ 已确定 |
| Session 隔离粒度（tokio task） | ✅ 已确定 |
| Session 持久化 schema | ✅ 已实现（PostgreSQL adapter + Redis adapter） |
| LLM provider 抽象接口 | ✅ 已实现（Anthropic/OpenAI/Google/Mistral/DeepSeek + Bedrock feature-gated） |
| API Gateway 协议选型 | 🟡 初步确定（客户端 API 采用 SSE + REST） |
| tenant crate | 🟡 核心功能已实现（并发配额、token/tool call 计量、session 生命周期、内置 extensions），CPU time 预算待实现 |
| observability crate | 🟡 核心功能已实现（tracing 初始化、Prometheus metrics、敏感数据脱敏），待与 agent-core/tenant/api-gateway 深度集成 |
| api-gateway | 🟡 核心功能已实现（REST API + SSE + HMAC 认证 + 限流），待与 observability 深度集成 |
| storage 集成测试 | ✅ 已实现（testcontainers 启动 PostgreSQL + Redis，并行测试 tenant/session ID 隔离） |
| 代码质量 | ✅ 修复（6 处 .unwrap() → .expect()，AskError 添加 thiserror，loop 中 TODO 修复） |
| TUI 客户端 | 🟡 核心功能已重构（ratatui + REST client + SSE 订阅），新增：输入队列（steer/followUp）、Bash 模式（`!command`/`!!command`）、外部编辑器（Ctrl+X）、命令面板解耦（Ctrl+Shift+P 任意状态）、模型循环切换（Ctrl+P/N）、Redo（Ctrl+Shift+-）、字符跳转（Ctrl+]）、CompactionSummary 消息类型。持续迭代中 |
| PromptBuilder 设计 | ✅ 核心类型 + SessionActor/AgentLoop 集成已完成（Phase 1）。Skills 以 `SkillsDirectory` fragment 注入，`render()` 产出完整 prompt。Hook 系统 `PromptMutation` 支持待 Phase 2 |

---

*本文件随项目演进持续更新。每次重大架构变更后，负责该变更的工程师需同步更新本文件相关章节，并在 git commit message 中注明 `docs: update AGENTS.md`。*
