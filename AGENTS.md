# AGENTS.md

> 本文件是项目的首要上下文文档。所有参与开发的 agent（包括 AI coding agent 和人类工程师）在开始任何任务前必须阅读本文件。

---

## 项目定义

**面向服务端多租户的 agent runtime & harness，以 Rust 实现，提供进程级会话隔离、资源配额、可观测性，以及内联的 hook 策略系统。**

本项目解决的核心问题：现有 agent 工具（如 pi.dev）以单用户单进程为设计前提，无法在服务端场景下为多个租户提供安全隔离、资源公平调度和生产级可观测性。本项目从架构层面重新设计，而非在现有工具上打补丁。

---

## 核心设计决策（ADR 摘要）

### ADR-001：Agent Loop 采用原生 Tool Use 协议

Agent loop 基于 LLM 原生 tool calling 协议：

```
UserMessage → AssistantMessage { ToolCall[] } → ToolResultMessage → ... → AssistantMessage { stop }
```

每个 turn 对应一次 LLM 响应，loop 在 `stop_reason = "stop"` 时终止。支持单个 AssistantMessage 内的并行 ToolCall（一次 LLM 响应多个工具调用）。

### ADR-002（已废除）：Extension 系统已内联至 agent-core

**v0.1.x 决策**：`extensions` crate 已被删除，原有的 `Extension` trait、`ExtensionActor`、`HookRouter`、`EventBus` 全部移除。

原 builtins（audit、path_guard、tool_guard、token_budget、content_filter）的逻辑已直接内联到 `agent-core/src/hook/default_dispatcher.rs` 中的 `DefaultHookDispatcher`。`HookDispatcher` trait 保留作为协议边界，但不再通过 Actor 运行时派发，而是直接函数调用。

> 未来若需第三方插件（WASM / RPC），将重新设计更轻量的插件运行时，而非恢复当前的 Extension Actor 模型。

### ADR-003：Hook 机制直接内联调用

原 Actor Mailbox + EventBus 混合模型已废除。当前 `HookDispatcher` 由 `DefaultHookDispatcher` 直接实现，所有 hook 为同步函数调用（无 Actor、无 oneshot、无 EventBus）。

| Hook 类型 | 调用方式 | 合并策略 |
|---|---|---|
| 阻断型拦截（`on_tool_call`、`on_before_compact`） | 直接函数调用 | first-block-wins |
| 链式拦截（`on_tool_result`、`on_context`、`on_before_agent_start`、`on_before_provider_request`、`on_after_provider_response`） | 直接函数调用 | 链式合并 |
| 观测型（`on_turn_end`、`on_agent_end`、`on_session_start` 等） | 直接函数调用 | 无需合并 |

**优势：**
- 零 Actor overhead（无 mpsc、无 oneshot、无 500ms/100ms 超时）
- panic 行为直接暴露（由 `AgentLoop`/`ToolExecutor` 统一捕获）
- 代码路径清晰，便于调试

**代价：**
- 失去 Extension 级 panic 隔离
- 失去第三方动态扩展能力（未来需重新设计）

### ADR-004：Session 隔离采用 tokio task 级别

每个租户 session 是一个独立的 tokio task 树，Session 之间不共享任何可变状态：

```
TenantSupervisor
  └── SessionActor (tenant_id, session_id)
        ├── AgentLoop            # 驱动 tool use loop（SessionActor 内部同步调用）
        ├── ToolExecutor         # 并行工具执行（SessionActor 内部同步调用）
        ├── DefaultHookDispatcher # 内联 hook 策略（直接函数调用）
        ├── CompactionActor      # 上下文压缩（SessionActor 内部同步调用）
        └── EventProcessor       # 事件处理（tokio::mpsc）
```

所有跨组件通信通过函数调用或 `tokio::mpsc`，禁止共享 `Arc<Mutex<_>>` 作为跨 session 状态。

> **实现说明**：`AgentLoop`、`ToolExecutor`、`CompactionActor` 和 `DefaultHookDispatcher` 均为 `SessionActor` 内部直接调用的组件，无 Actor mailbox。这一设计最大程度简化了 session 内部状态流转。

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
    hook/            #   HookDispatcher trait、DefaultHookDispatcher、*Ctx、*Mutation
    space.rs         #   AgentSpace 统一目录抽象
    skills/          #   Skill 扫描、加载、注入
    persistence/     #   持久化边界（SessionStore trait、SessionEntry）
    prompt/          #   PromptBuilder、PromptMutation
    utils/           #   工具与选项
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
api-gateway → tenant → agent-core → ai-provider
                   ↓
              storage / observability
```

---

## AgentSpace 统一目录结构

所有运行时数据（工作空间、配置、缓存、日志、临时文件、skills）统一在一个根目录下管理，默认路径由平台决定：

- **macOS**: `~/Library/Application Support/pandaria/`
- **Linux**: `~/.local/share/pandaria/`
- **覆盖**: `PANDARIA_SPACE_ROOT` 环境变量

### 目录布局

```
{pandaria_root}/
  ├── config/              # 配置文件
  │     └── tui/
  │           └── config.toml
  ├── cache/               # LLM 响应缓存等
  ├── logs/                # 文件日志（tracing-appender）
  ├── temp/                # 临时文件
  ├── skills/              # 全局 skill 定义文件
  └── workspaces/
        └── {tenant_id}/   # 租户级工作空间
```

> `workspaces/{tenant_id}/` 是 agent 文件工具（如 read_file / write_file）的沙箱，**不存储 session 状态**。session 状态（消息历史、compaction 结果）通过 `SessionStore` 持久化到 PostgreSQL / Redis。

### 使用方式

- **PathGuard**: `AgentSpace::workspace_for(tenant_id)` 作为允许的文件访问前缀
- **Skills Scanner**: 默认扫描 `AgentSpace::skills_dir()`，可通过 `PANDARIA_SKILLS_DIR` 覆盖
- **TUI**: 配置默认读取/写入 `{root}/config/tui/config.toml`，临时文件使用 `{root}/temp/`
- **代码**: `AgentSpace::from_env_or_default()` 获取实例，调用 `ensure_dirs()` 确保目录存在

---

## 关键约束

### 并发模型

- 所有异步代码使用 `tokio`，禁止 `std::thread::sleep` 等阻塞调用出现在 async 上下文中。
- CPU 密集型操作（大文本压缩、序列化）使用 `tokio::task::spawn_blocking`。
- Hook 调用为直接函数调用，无超时边界。各策略内部若有异步操作（如 IO），自行管理超时。

### 错误处理

- 所有跨 crate 的错误类型使用 `thiserror` 定义。
- 非测试代码应最小化 `.unwrap()`，优先使用 `?` 或显式 `expect("reason")`。当前代码库尚有 190 个非测试 unwrap 待逐步清理（agent-core: 94, ai-provider: 71, api-gateway: 10, tui: 15）。
- HookDispatcher panic 由调用方（`AgentLoop` / `ToolExecutor`）统一捕获并记录。
- LLM API 调用必须实现指数退避重试（最多 3 次），并在 tracing span 中记录重试次数。

### 安全约束

- `tenant_id` 必须在所有 tracing span 和日志中出现，禁止无 tenant 上下文的操作日志。
- 工具执行时的文件系统访问必须经过路径校验（由 `path_guard` 在 `on_tool_call` 中拦截），禁止访问 `AgentSpace::workspace_for(tenant_id)` 以外的路径。
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
| `pi.on("tool_call")` | `HookDispatcher::on_tool_call` → 直接函数调用 |
| `pi.on("turn_end")` | `HookDispatcher::on_turn_end` → 直接函数调用 |
| `pi.registerTool()` | `AgentToolRef` 直接注册 |
| `session.compact()` | `CompactionActor::compact()` |
| Extension npm 包 | 已删除 Extension 系统，未来如需插件将重新设计 |
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
| Extension 模型（Rust trait） | ❌ 已删除（v0.1.x 内联为 DefaultHookDispatcher） |
| Hook 机制（直接函数调用） | ✅ 已确定 |
| Session 隔离粒度（tokio task） | ✅ 已确定 |
| Session 持久化 schema | ✅ 已实现（PostgreSQL adapter + Redis adapter） |
| LLM provider 抽象接口 | ✅ 已实现（Anthropic/OpenAI/Google/Mistral/DeepSeek + Bedrock feature-gated） |
| API Gateway 协议选型 | 🟡 初步确定（客户端 API 采用 SSE + REST） |
| tenant crate | 🟡 核心功能已实现（并发配额、token/tool call 计量、session 生命周期），CPU time 预算待实现 |
| observability crate | 🟡 核心功能已实现（tracing 初始化、Prometheus metrics、敏感数据脱敏），待与 agent-core/tenant/api-gateway 深度集成 |
| api-gateway | 🟡 核心功能已实现（REST API + SSE + HMAC 认证 + 限流），待与 observability 深度集成 |
| storage 集成测试 | ✅ 已实现（testcontainers 启动 PostgreSQL + Redis，并行测试 tenant/session ID 隔离） |
| 代码质量 | 🟡 部分修复（6 处 .unwrap() → .expect()，AskError 添加 thiserror，loop 中 TODO 修复）。当前非测试 unwrap 共 190 个，待逐步清理 |
| TUI 客户端 | 🟡 核心功能已重构（ratatui + REST client + SSE 订阅），新增：输入队列（steer/followUp）、Bash 模式（`!command`/`!!command`）、外部编辑器（Ctrl+X）、命令面板解耦（Ctrl+Shift+P 任意状态）、模型循环切换（Ctrl+P/N）、Redo（Ctrl+Shift+-）、字符跳转（Ctrl+]）、CompactionSummary 消息类型。持续迭代中 |
| PromptBuilder 设计 | ✅ Phase 1 & 2 已完成。核心类型 + SessionActor/AgentLoop 集成 + Hook 系统 `PromptBuilder` 接入。`BeforeAgentStartMutation` / `ProviderRequestMutation` 新增 `prompt_mutation: Option<PromptMutation>` 字段；legacy `system_prompt: Option<PromptBuilder>` 保留向后兼容，替换后框架自动重新注入 `SkillsDirectory`。`inject_skills_into_builder` 辅助函数提取至 `skills/injector.rs`。 |
| AgentSpace 统一目录 | ✅ 已实现（`agent-core/src/space.rs`）。统一根目录（默认 `~/.local/share/pandaria`），含 config/cache/logs/temp/skills/workspaces 子目录。PathGuard、Skills Scanner、TUI 均已接入。`PANDARIA_SPACE_ROOT` 环境变量可覆盖根目录。

---

*本文件随项目演进持续更新。每次重大架构变更后，负责该变更的工程师需同步更新本文件相关章节，并在 git commit message 中注明 `docs: update AGENTS.md`。*
