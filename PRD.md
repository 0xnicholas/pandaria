# Pandaria — 产品需求文档

## 产品愿景与定位

### 市场背景

2025 年，AI agent 领域涌现大量框架和工具，但基础设施层存在明显缺口：

- **LLM 编排框架**（LangChain、Semantic Kernel、CrewAI）聚焦 Prompt → Tool → Response 的流程编排，但将租户隔离、资源调度和可观测性完全交由使用者自行解决。
- **本地 Agent 工具**（Claude Code、pi.dev、OpenAI Codex CLI）以开发者个人终端为核心场景，单用户单进程，天然不具备多租户服务端能力。
- **云厂商 Agent 服务**（AWS Bedrock Agents、Google Vertex AI Agent Builder）提供托管式 agent，但平台锁定、定制受限，且 Extension 生态封闭。

要构建一个面向多客户的服务端 AI agent 产品，团队需要在现有框架之上从头搭建隔离、调度、持久化、可观测等基础设施——这正是 Pandaria 填补的空白。

### 产品愿景

一个 Rust 实现的、为多租户 AI agent 平台提供安全隔离、资源调度和生产级可观测性的服务端 runtime，让团队不再从零拼凑基础设施，专注构建自己的 agent 产品。

### 差异化定位

| 维度 | Pandaria 的特点 |
|---|---|
| 多租户优先 | 架构层面（tokio task 隔离），而非应用层拼凑 |
| 安全 Extension | Actor 模型隔离 + panic 捕获 + 超时保护，Extension 崩溃不影响 agent loop |
| 编译时安全 | Rust 类型系统消除数据竞争 + 内存安全问题 |
| 部署独立 | 单个二进制自包含，不依赖 Python / Node 运行时生态 |

---

## 目标用户

### 平台开发团队

- **背景**：有后端基础设施经验的团队（2-8 人），熟悉 Rust 或愿意采用 Rust。正在构建面向企业客户的内部 AI agent 平台，或面向开发者的 agent SaaS。
- **核心痛点**：
  - 客户 A 的 agent 不能看到客户 B 的数据，手动实现租户隔离易出错
  - 无法按客户控制 token 消耗和并发上限，成本不可控
  - session 内存状态随服务重启丢失，用户体验差
  - Extension 沙箱缺失，第三方代码的 panic 可能拖垮整个服务
- **使用场景**：在自己的服务端应用中嵌入 Pandaria 作为 agent 运行时，通过 Rust API 管理 session、注册 Extension、接入自有前端。
- **最终目标**：用最少的自研基础设施代码，交付一个安全、可控、可观测的多租户 agent 平台。

### Extension 开发者

- **背景**：熟悉 Rust 的工程师，为平台开发自定义工具或行为策略（如文件操作、代码执行、审计日志）。
- **核心痛点**：
  - Extension 的生命周期管理（何时初始化、如何获取 session 上下文、如何安全清理）
  - 工具调用参数的类型安全校验
  - Hook 的执行顺序和合并语义不清晰
- **使用场景**：实现 `Extension` trait，通过 `ExtensionManager` 注册到平台。
- **最终目标**：聚焦业务逻辑，不关心多租户隔离、超时保护等基础设施。

### 终端用户

终端用户不直接使用 Pandaria，而是通过平台方构建的产品界面与 agent 交互。按使用方式分为两类：

**平台集成方（开发者）**

- **背景**：平台方团队中的后端/前端工程师，通过 Pandaria 的 API 或 Rust crate 构建 agent 产品。
- **核心痛点**：
  - 需要验证 agent loop 行为、Extension hook 执行顺序和工具适配
  - 缺少便捷的调试入口，只能通过日志排查问题
- **使用场景**：通过 TUI 或直接调用 Rust API 创建测试 session，发送 prompt 并观察 agent 行为。
- **最终目标**：快速验证和调试，确保 agent 行为符合预期后再接入生产界面。

**产品终端用户（普通用户）**

- **背景**：通过平台方提供的 Web / CLI / API 界面与 agent 对话的用户，无需了解 Pandaria 的存在。
- **核心痛点**：agent 崩溃或响应丢失时需要重新开始对话。
- **使用场景**：创建 session、发送 prompt、查看 agent 执行过程和结果，期望对话历史持久化。
- **最终目标**：可靠、流畅的 agent 对话体验，不会因服务重启丢失上下文。

---

## 竞争分析

### LangChain

| 维度 | LangChain | Pandaria |
|---|---|---|
| 定位 | LLM 应用开发框架 | 服务端多租户 agent runtime |
| 语言 | Python、JavaScript | Rust |
| 多租户 | 不提供，需应用层自行实现 | 内置 per-tenant tokio task 隔离 |
| Session 持久化 | 通过第三方集成（LangSmith 等） | 内置 SessionStore trait + Redis/PG 适配器（计划中） |
| Extension 模型 | Tool / Callback 装饰器 | Rust trait + Actor Mailbox 隔离，19 个 hook 点 |
| Extension 安全 | 进程内（异常可传播） | Actor 隔离 + panic 捕获 + 超时保护 |
| 资源配额 | 无内置机制 | 内置 per-tenant 并发 / token / CPU 配额（计划中） |
| 可观测性 | 依赖 LangSmith（商业产品） | 基于 tracing，自带 per-tenant span（计划中） |
| 部署模型 | 嵌入 Python/JS 应用 | 独立 Rust 二进制，自包含 |
| 运行环境依赖 | Python / Node 运行时 | 仅需 Rust 编译产物 |

**总结**：LangChain 是通用 LLM 编排框架，Pandaria 聚焦于「将 agent 作为服务交付」的场景。两者在目标用户和实施方式上互补而非替代——LangChain 可嵌入在 Pandaria 的 Extension 中使用。

**选型建议**：如果你在构建一个应用内嵌的 LLM 功能（单用户、无需多租户），LangChain 更合适。如果你在构建一个面向多客户的服务端 agent 平台，Pandaria 直接提供隔离、调度和持久化基础。

### Pi.dev

| 维度 | Pi.dev | Pandaria |
|---|---|---|
| 定位 | 本地开发者 agent CLI | 服务端多租户 agent runtime |
| 语言 | Node.js / TypeScript | Rust |
| 运行模式 | 单用户终端进程 | 共享服务进程 + per-session tokio task |
| 多租户 | 不支持，设计前提为单用户 | 核心设计目标 |
| Extension 模型 | `pi.on("event")` / `pi.registerTool()`（npm 包） | Rust trait + Actor Mailbox + EventBus（编译期注册） |
| Extension 安全 | 进程内执行（同步/异步回调） | Actor 隔离 + panic 不传播 + 超时降级 |
| 热更新 | `/reload` 支持热更新 Extension | 不支持（服务端无此需求） |
| 持久化 | 本地文件 | Redis / PostgreSQL（计划中） |
| 可观测性 | 终端输出 | tracing span + per-tenant metrics（计划中） |
| 协议 | 自定义 Tool Use 协议（`assistant` ↔ `tool_output`） | LLM 原生 tool calling 协议（`AssistantMessage { ToolCall[] }`） |
| 部署 | 本地安装 / npm | 服务端 self-hosted |

**Pi → Pandaria 的概念对照**：`AgentSession` → `SessionActor`；`pi.on("tool_call")` → `Extension::on_tool_call`；`pi.registerTool()` → `Extension::tools()`；`session.compact()` → `CompactionActor::compact()`。

**总结**：Pandaria 的 agent loop 语义以 pi.dev 为参考实现，但架构目标完全不同。pi.dev 服务于个人开发者，Pandaria 服务于平台构建者。pi.dev 的 Extension 通过 EventBus 运行在单进程中，Pandaria 的 Extension 通过 Actor 模型提供更强隔离。

**选型建议**：如果你是一名独立开发者，在本地终端中与 agent 协作写代码，pi.dev 更轻量直接。如果你是一个团队，需要将 agent 能力以服务形式开放给多个租户，Pandaria 提供开箱即用的多租户基础设施。

---

## 用户故事

### P0 — MVP

**US-01** 租户会话隔离
> 作为平台开发者，我能为每个租户创建隔离的 agent session，使得不同租户的数据和状态互不可见。

- **验收标准**：
  - GIVEN 租户 A 和租户 B 各有一个活跃 session
  - WHEN 租户 A 的 agent 执行工具并生成结果
  - THEN 租户 B 的 session 无法通过任何 API 访问租户 A 的消息历史或工具结果
- **优先级理由**：多租户隔离是产品存在的根本前提。隔离失败意味着产品不可用。

**US-02** Extension 注册
> 作为 Extension 开发者，我能通过 Rust trait 注册自定义 Extension（工具注册、hook 拦截），并在编译期完成集成。

- **验收标准**：
  - GIVEN 一个实现了 `Extension` trait 的类型
  - WHEN 通过 `ExtensionManager::register()` 注册并 `spawn_all()`
  - THEN Extension 的工具出现在 agent 可用工具列表中，hook 方法在对应生命周期被调用
- **优先级理由**：Extension 系统是产品区别于通用 agent 框架的核心能力。

**US-03** 终端对话（开发/调试）
> 作为平台开发者，我能通过 TUI 创建 session、发送 prompt 并实时看到 agent 的流式回复和工具调用执行过程，用于开发调试。

- **验收标准**：
  - GIVEN TUI 客户端已启动
  - WHEN 用户输入 "帮我读取 src/main.rs 的内容"
  - THEN 界面实时展示 agent 的思考过程、tool call 参数和执行结果
- **优先级理由**：TUI 是开发阶段的交互验证和调试入口，非面向终端用户的生产界面。

**US-04** 多 Provider 接入
> 作为平台开发者，我能接入 Anthropic、OpenAI、Google、Mistral 四种 LLM provider，并自动处理重试、上下文溢出恢复。

- **验收标准**：
  - GIVEN 环境变量设置了对应 provider 的 API Key
  - WHEN 创建使用该 provider 的 session 并发送 prompt
  - THEN agent 正常执行 tool use loop；若 API 返回 429，自动指数退避重试
- **优先级理由**：LLM 接入是 agent 运行的基础依赖。

**US-05** 内置 Extension 开箱即用
> 作为平台开发者，内置的 rate-limit 和 tool-guard Extension 直接可用，无需额外开发即可控制工具调用频率和权限。

- **验收标准**：
  - GIVEN `RateLimitExtension` 配置为每分钟 10 次，`ToolGuardExtension` 配置 deny list 包含 "dangerous_command"
  - WHEN agent 在第 11 次工具调用或尝试调用 "dangerous_command" 时
  - THEN 工具调用被阻断，返回原因
- **优先级理由**：风控和安全是平台运营的基础能力。

### P1 — 生产就绪

**US-06** 租户资源配额
> 作为平台开发者，我能为每个租户配置 token 消耗上限、并发 session 数和 CPU 时间预算，超出配额自动拒绝。

- **验收标准**：
  - GIVEN 租户 A 的月 token 配额为 100 万
  - WHEN 租户 A 的本月 token 消耗达到 100 万
  - THEN 新请求返回配额耗尽错误，不影响其他租户
- **优先级理由**：成本控制是多租户商业化的前提。

**US-07** Session 持久化与恢复
> 作为平台开发者，agent session 的状态能持久化到外存，服务重启后自动恢复。

- **验收标准**：
  - GIVEN 一个活跃 session 的对话历史已持久化到 Redis
  - WHEN 服务重启并恢复该 session
  - THEN agent 携带完整的压缩后上下文继续对话
- **优先级理由**：生产环境服务重启不可避免，丢失状态是严重体验缺陷。

**US-08** API 接入层
> 作为平台开发者，我通过 gRPC 或 WebSocket 接入 Pandaria，实现自己的前端界面。

- **验收标准**：
  - GIVEN api-gateway 已启动
  - WHEN 客户端通过 WebSocket 发送 prompt
  - THEN 通过 SSE 事件流收到实时响应和工具执行进度
- **优先级理由**：TUI 仅供开发调试，生产环境需要标准 API 接入。

**US-09** 认证与限流
> 作为平台开发者，API 接口带有认证和 per-tenant 限流保护。

- **验收标准**：
  - GIVEN 租户 B 的 API Key
  - WHEN 租户 B 在 1 秒内发起 100 个请求（超过限流阈值）
  - THEN 超出阈值的请求被拒绝，返回 429
- **优先级理由**：API 安全是生产部署的最低门槛。

### P2 — 规模化

**US-10** 水平扩展
> 作为平台开发者，单个 Pandaria 实例能在无状态容器中水平扩展，session 可在节点间迁移。

- **验收标准**：
  - GIVEN 两个 Pandaria 节点共享同一 Persistence 后端
  - WHEN 节点 A 宕机
  - THEN 节点 A 上的 session 可迁移到节点 B 并恢复执行
- **优先级理由**：规模化部署需求，但可依托持久化能力降级实现。

**US-11** 云厂商托管模型
> 作为平台开发者，我能接入 AWS Bedrock 等云厂托管模型。

- **验收标准**：
  - GIVEN AWS 凭证配置正确
  - WHEN 创建使用 Bedrock provider 的 session
  - THEN agent 通过 Bedrock Converse API 正常执行
- **优先级理由**：企业客户常要求使用云厂商托管模型以满足合规需求。

**US-12** 跨语言 Extension
> 作为 Extension 开发者，我可以用 WASM 或 RPC 方式编写 Extension，无需使用 Rust 编译。

- **验收标准**：
  - GIVEN 一个编译好的 WASM 模块
  - WHEN 通过 `ExtensionManager` 加载
  - THEN 该模块的工具被注册到 agent 可用工具列表中
- **优先级理由**：降低 Extension 开发门槛，扩大生态。

---

## 功能需求

### MVP（当前开发阶段）

**F-01** LLM Provider 抽象层
> 支持 Anthropic、OpenAI、Google、Mistral 四种 Provider，统一的 `LlmProvider` trait 接口。

- **验收标准**：每种 Provider 均能通过对应 API 完成 SSE 流式请求和正常的 tool use loop。
- **技术依赖**：`reqwest`
- **设计参照**：[ADR-001](./AGENTS.md) — 原生 tool calling 协议
- **状态**：🟡 开发中

**F-02** Agent Tool Use Loop
> 基于 LLM 原生 tool calling 协议的 turn-by-turn 循环，支持单 AssistantMessage 内并行 ToolCall。

- **验收标准**：agent 能正确处理 `Stop`、`ToolUse`、`Length`、`Error`、`Aborted` 五种终止原因，并在 `ToolUse` 时执行工具调用后继续下一轮。
- **技术依赖**：F-01
- **可选依赖**：F-07（CompactionActor，增强上下文溢出恢复能力）
- **设计参照**：[ADR-001](./AGENTS.md) — 原生 tool calling 协议
- **状态**：🟡 开发中

**F-03** SessionActor
> per-tenant session 生命周期管理器，持有消息历史、工具集、steer/follow-up 队列。

- **验收标准**：
  - 不同 `tenant_id` 的 SessionActor 之间零共享状态
  - 支持 `prompt()` 开始对话、`abort()` 取消执行
- **技术依赖**：F-02
- **设计参照**：[ADR-004](./AGENTS.md)
- **状态**：🟡 开发中

**F-04** Extension Trait
> 19 个 hook 方法 + `tools()` + `execute_tool()` 的完整 Rust trait。

- **验收标准**：Extension 能通过实现 trait 注册工具、拦截 tool call、修改结果、观测事件。
- **设计参照**：[ADR-002](./AGENTS.md)
- **状态**：🟡 开发中

**F-05** HookRouter（内部实现）
> 实现 `HookDispatcher`，按消息语义路由到 Actor Mailbox（阻断型、链式）或 EventBus（观测型）。为 F-06 和后续 Extension 提供路由基础设施。

- **验收标准**：
  - 阻断型 hook：first-block-wins，首个 Block 立即返回
  - 链式 hook：逐 handler 叠加修改，返回累积结果
  - 观测型 hook：并发广播，100ms 超时静默丢弃
- **技术依赖**：F-04
- **设计参照**：[ADR-003](./AGENTS.md)
- **状态**：🟡 开发中

**F-06** 内置 Extension
> AuditExtension（审计日志）、RateLimitExtension（频率限制）、ToolGuardExtension（访问控制）。

- **验收标准**：
  - Audit：每个 tool call 和 turn 产生一条 `pandaria.audit` tracing 记录
  - RateLimit：超过滑动窗口上限的 tool call 被 Block
  - ToolGuard：denied_tools 中的工具被 Block，非空 allowed_tools 中未列出的工具被 Block
- **技术依赖**：F-04、F-05
- **状态**：🟡 开发中

**F-07** CompactionActor（上下文压缩）
> 自动 compaction（上下文压缩），通过 LLM 生成结构化摘要替换历史消息。

- **验收标准**：
  - 当上下文 token 超过阈值时自动触发压缩
  - 压缩后保留最近 N 条消息 + 摘要（压缩率 > 60%）
  - 压缩需经 `on_before_compact` hook 审查
- **技术依赖**：F-01
- **设计参照**：pi.dev 的 `session.compact()` 语义
- **状态**：🟡 开发中

**F-08** RecoveryStateMachine（内部实现）
> 错误恢复状态机，评估 `RecoveryAction`（Continue / RetryAfterBackoff / RetryAfterCompaction / Abort）。为 Agent Loop 提供错误恢复决策逻辑。

- **验收标准**：
  - 上下文溢出 → RetryAfterCompaction（最多 1 次）
  - RateLimited / Overloaded / Timeout → RetryAfterBackoff（最多 3 次）
  - 其他错误 → Abort
- **技术依赖**：F-01（overflow 检测）
- **状态**：🟡 开发中

**F-09** TUI 客户端
> 独立二进制 `pandaria-tui`，ratatui + crossterm 实现。

- **验收标准**：
  - 创建/切换 session、输入 prompt、实时显示流式回复
  - Markdown 渲染 + 语法高亮 + 自动补全
- **技术依赖**：REST + SSE 通信
- **状态**：🟡 开发中

**F-10** JSON Schema 工具参数校验
> `validate_tool_arguments()` — 类型强制（string→number、string→boolean 等）+ 缓存编译的 schema。

- **验收标准**：符合规范的参数直接通过，可强转的参数强转后通过，非法参数返回具体错误。
- **技术依赖**：F-01（类型系统）
- **状态**：🟡 开发中

**F-11** 流式 JSON 修复
> `StreamingJsonParser` — 渐进解析 + 启发式修复（unclosed strings、trailing commas、single quotes、unbalanced brackets）。

- **验收标准**：常见的流式截断 JSON 能被正确修复并完成解析。
- **技术依赖**：F-01
- **状态**：🟡 开发中

**F-12** 模型注册表
> `ModelRegistry` — 47+ 模型注册 + `calculate_cost()` 成本计算。

- **验收标准**：精确匹配 model name 返回对应 provider 和 cost 信息。
- **技术依赖**：F-01
- **状态**：🟡 开发中

### V1（下一阶段）

**F-13** `api-gateway` crate — gRPC / WebSocket 接入层
- **验收标准**：客户端可通过 WebSocket 创建 session、发送 prompt、接收 SSE 事件流。
- **技术依赖**：F-03（SessionActor）、F-14（认证）
- **优先级**：P0

**F-14** API 认证 — API Key / JWT
- **验收标准**：无有效 API Key 的请求返回 401，过期/无效 JWT 返回 401。
- **技术依赖**：F-13
- **优先级**：P0

**F-15** Per-tenant 请求限流
- **验收标准**：每个租户在滑动窗口内的请求数不超过配置上限。
- **技术依赖**：F-13、F-16
- **优先级**：P1

**F-16** `tenant` crate — Session 注册表 + 配额管理
- **验收标准**：注册表记录所有活跃 session，每个租户的并发 session 数不超过配额。
- **技术依赖**：F-03
- **设计参照**：[ADR-005](./AGENTS.md)
- **优先级**：P0

**F-17** 租户资源配额 — 并发上限、CPU budget
- **验收标准**：超出配额的新 session 创建请求被拒绝。
- **技术依赖**：F-16
- **优先级**：P1

**F-18** Token 消耗计量
- **验收标准**：每个 session 的 token 消耗与租户消费绑定，超出配额拒绝新请求。
- **技术依赖**：F-01（Usage）、F-16
- **优先级**：P1

**F-19** `storage` crate — SessionStore Redis 适配器
- **验收标准**：session 消息历史可写入 Redis 并完整恢复。
- **技术依赖**：F-03（SessionEntry）、F-07（Compaction）
- **优先级**：P0

**F-20** `storage` crate — SessionStore PostgreSQL 适配器
- **验收标准**：同 F-19，后端为 PostgreSQL。
- **技术依赖**：F-21（schema）
- **优先级**：P1

**F-21** Session 持久化 schema 设计
- **验收标准**：schema 支持消息历史、compaction 摘要、元数据的完整序列化与反序列化。
- **技术依赖**：F-03
- **优先级**：P0

### V2（远期）

**F-22** `observability` crate — tracing 集成、per-tenant metrics
- **验收标准**：所有 span 携带 `tenant_id` / `session_id`，per-tenant tool call 耗时、token 消耗、错误率可通过 metrics endpoint 查询。
- **设计参照**：[ADR-005](./AGENTS.md) — 多租户基础能力
- **优先级**：P2（依赖 api-gateway 提供 metrics endpoint，且基础 tracing 已通过 `tracing` crate 在各模块中内联使用）

**F-23** 分布式追踪（跨节点 propagation）
- **验收标准**：同一 session 在节点 A 创建、节点 B 恢复后，trace 链路完整衔接。
- **优先级**：P2

**F-24** 水平扩展 — session 跨节点迁移
- **验收标准**：利用持久化能力，session 在任意节点恢复执行。
- **技术依赖**：F-19/F-20（持久化）
- **优先级**：P2

**F-25** AWS Bedrock provider 正式支持
- **验收标准**：通过 Bedrock Converse API 完成完整 agent loop。
- **技术依赖**：F-01
- **优先级**：P2

**F-26** WASM Extension 运行时
- **验收标准**：加载 wasm32-wasi 模块，调用其工具函数并获取结果。
- **设计参照**：[ADR-002](./AGENTS.md) — 预留边界
- **优先级**：P2

**F-27** RPC Extension 边界（gRPC）
- **验收标准**：外部 gRPC 服务实现 Extension 协议，由 Pandaria 调度调用。
- **优先级**：P2

---

## 核心数据模型

```
Message ──────────────── SessionEntry ──────────────── SessionActor.entries

Message                         SessionEntry
├── UserMessage                 ├── Message { id, message: Message }
│   └── content: Vec<Content>   └── Compaction { id, summary,
├── AssistantMessage                        first_kept_entry_id,
│   ├── content: Vec<Content>               tokens_before,
│   ├── provider, model                     details, from_extension,
│   ├── usage: Usage                        timestamp }
│   ├── stop_reason: StopReason
│   └── response_id                   SessionActor
├── ToolResultMessage                 ├── entries: Vec<SessionEntry>
│   ├── tool_call_id                  ├── tools: Vec<AgentToolRef>
│   ├── tool_name                     ├── steer_queue: mpsc
│   ├── content                       ├── follow_up_queue: mpsc
│   ├── details                       ├── event_listeners
│   ├── is_error                      └── recovery: RecoveryStateMachine
│   └── timestamp
                                  AgentTool
Content (enum)                    ├── name, description
├── Text                          ├── parameters: serde_json::Value
├── Image                         ├── execution_mode: Sequential|Parallel
├── Thinking                      └── execute() → AgentToolResult
└── ToolCall
    ├── id, name, arguments       ToolExecutor
    └── thought_signature         └── 管线上绑:
                                      on_tool_call → execute → on_tool_result

ToolDef                           Extension (共 19 个 hook 方法，仅列代表性接口)
├── name                          ├── name()
├── description                   ├── tools() → Vec<ToolDef>
└── parameters: serde_json::Value ├── on_tool_call() → (HookDecision, ToolCallMutation)
                                  ├── on_tool_result() → ToolResultMutation
LlmProvider                       ├── on_context() → ContextMutation
├── provider_name()               ├── on_before_agent_start() → BeforeAgentStartMutation
├── models() → Vec<String>        ├── on_before_provider_request() → ProviderRequestMutation
├── api_for() → Api               ├── on_after_provider_response() → ProviderResponseMutation
└── stream() → AssistantMessageEventStream
                                  └── execute_tool() → AgentToolResult
                                  ExtensionActor (per Extension)
                                  ├── mpsc mailbox → blocking/chain hooks
                                  └── broadcast recv → observational hooks

HookDispatcher (trait)            ExtensionHandle
├── on_tool_call()                └── ask() pattern (oneshot reply)
├── on_tool_result()
├── on_context()                  ExtensionManager
├── on_before_compact()           ├── register(Arc<dyn Extension>)
├── on_turn_end()                 ├── spawn_all() → (HookRouter, Vec<AgentToolRef>)
├── on_agent_end()                └── shutdown_all()
└── ...（共 19 个 hook 方法）
```

---

## 风险与缓解

| 风险 | 影响 | 概率 | 缓解措施 |
|---|---|---|---|
| LLM API 变更导致 Provider 适配断裂 | 特定 Provider 不可用，但其他 Provider 仍可用 | 中 | 每个 Provider 独立实现 `LlmProvider` trait，故障不传染；integration test 覆盖各 Provider |
| Extension Actor 死锁 | 特定 Extension 不可用，但 agent loop 继续 | 低 | 禁止在 Mailbox 处理中发起新的同步 ask；所有 oneshot reply 带 500ms 超时；panic 由 JoinHandle 捕获 |
| 上下文压缩遗漏关键信息 | 后续 turn 的回答质量下降 | 中 | Compaction 保留最近 N 条消息作为 raw context，仅旧消息被摘要替代；压缩内容需经 `on_before_compact` hook 审查 |
| 共享持久化后端成为瓶颈 | 大规模 session 并发时读写延迟增加 | 中 | SessionStore 异步 trait，持久化操作 fire-and-forget（不阻塞 loop）；支持 Redis（低延迟）和 PG（强一致性）双后端 |
| 单节点资源耗尽 | 并发 session 数超过 CPU/内存容量 | 中 | Per-tenant 并发上限限制 + 持久化支持跨节点迁移（V2）；tokio task 本身是轻量级隔离单元 |

---

## 里程碑与发布计划

### M1 — MVP（当前）

**目标**：完成核心 agent loop + Extension 系统，可独立运行并通过 TUI 交互。

**交付物**：
- `ai-provider` crate：4 个 Provider、SSE 流式、重试/校验/修复
- `agent-core` crate：AgentLoop、SessionActor、ToolExecutor、CompactionActor、HookDispatcher trait
- `extensions` crate：Extension trait、Actor Mailbox、EventBus、HookRouter、3 个内置 Extension
- `tui` crate：多 session TUI 客户端

**状态**：🟡 开发中

### M2 — V1（生产就绪）

**目标**：服务端可部署，支持多租户、持久化和 API 接入。

**交付物**：
- `api-gateway` crate：gRPC / WebSocket 接入、认证、限流
- `tenant` crate：调度器、配额管理、Session 注册表
- `storage` crate：Redis + PostgreSQL SessionStore 实现
- Session 持久化 schema

**状态**：🔲 计划中

### M3 — V2（规模化）

**目标**：水平扩展、可观测性升级、跨语言 Extension。

**交付物**：
- `observability` crate：per-tenant tracing metrics
- 水平扩展：跨节点 session 迁移
- AWS Bedrock provider 正式支持
- WASM / RPC Extension 运行时

**状态**：🔲 计划中

---

## 非功能需求

### 性能

| 指标 | 目标 |
|---|---|
| 单 session 端到端延迟（不含 LLM 调用） | < 10ms |
| 单节点并发 session 数 | ≥ 1000 |
| Extension hook 超时（拦截型 / 链式） | 500ms，超时默认 Continue / 跳过 |
| Extension hook 超时（观测型） | 100ms，超时静默丢弃 |
| LLM 重试策略 | 最多 3 次，指数退避 100ms 基础延迟 |
| Tool execution 超时 | 不设全局超时（由工具自行控制），spawn_blocking 用于 CPU 密集型操作 |

### 安全

| 约束 | 说明 |
|---|---|
| `tenant_id` 必现在日志 | 所有 tracing span 和日志必须包含 `tenant_id`，禁止无租户上下文的操作日志 |
| 文件系统路径隔离 | Extension 文件系统访问限制在 `/workspace/{tenant_id}/` 以内 |
| API Key 屏蔽 | LLM API Key 不得出现在日志、tracing span、错误消息或 panic 信息中 |
| Extension panic 隔离 | Extension panic 不得传播到 agent loop，通过 `tokio::task` JoinHandle 捕获 |
| 认证 | API 接入需 API Key 或 JWT 认证（V1） |
| 限流 | API 接入需 per-tenant 限流保护（V1） |

### 可靠性

| 约束 | 说明 |
|---|---|
| 禁止 unwrap | 非测试代码禁止 `.unwrap()`，使用 `?` 或 `expect("reason")` |
| 异步阻塞隔离 | 禁止 `std::thread::sleep` 等阻塞调用出现在 async 上下文中，CPU 密集型操作使用 `tokio::task::spawn_blocking` |
| 错误类型 | 所有跨 crate 错误使用 `thiserror` 定义 |
| LLM 重试 | 指数退避，最多 3 次，可选 `max_retry_delay_ms` 上限 |
| 上下文恢复 | 溢出检测后自动触发 compaction and retry（最多 1 次） |

### 合规

| 约束 | 说明 |
|---|---|
| 租户数据隔离 | 认证权鉴必须验证请求方身份与目标 tenant_id 的一致性 |
| 审计日志 | AuditExtension 提供观测型审计记录，输出至 tracing journal |

### 可维护性

| 约束 | 说明 |
|---|---|
| 依赖方向单向 | 严格遵守 `api-gateway → tenant → extensions → agent-core → ai-provider` 方向，禁止反向依赖 |
| 公开 API 文档 | 所有公开 API 必须有 `///` 文档注释 |
| 新 crate README | 新 crate 必须包含描述职责、公开接口和边界的 README.md |
| 集成测试 | 集成测试使用 `testcontainers` 启动依赖，禁止测试依赖外部网络 |

---

## 成功指标

| 指标 | 衡量方式 | 目标 |
|---|---|---|
| Extension 隔离性 | Extension panic 后 session 是否仍正常完成 | 100% |
| Extension hook 超时降级率 | 拦截型/链式 hook 超时后被跳过的比例 | < 0.1% |
| Extension panic 恢复率 | Extension panic 被捕获后 agent loop 继续正常完成的比例 | 100% |
| 多租户安全性 | 一个租户能否读取另一个租户的 session 数据 | 0 泄漏 |
| Provider 可用性 | 重试后 LLM 调用成功率 | > 99.9% |
| 上下文压缩效果 | 压缩后 token 减少比例 | > 60% |
| 工具调用校验 | JSON Schema 校验准确率 | 100% |

---

## 范围界定

### In scope

- 服务端多租户 agent runtime（本仓库）
- Extension 系统（Rust 编译期注册，未来 WASM / RPC）
- 内置 Extension（audit、rate-limit、tool-guard）
- LLM provider 集成（Anthropic、OpenAI、Google、Mistral）
- TUI 客户端（独立二进制）
- API 接入层（gRPC / WebSocket）

### Out of scope

- 前端 Web UI（TUI 为开发/调试工具，非生产界面）
- LLM 模型微调 / 训练
- Agent skill marketplace（Extension 分发平台）
- 第三方云服务托管 SaaS（仅提供 self-hosted runtime）
- 多语言 Extension SDK（当前仅 Rust，未来 WASM 可间接支持）
- 本地模型推理（仅对接外部 LLM API）

---

## 参考资料

- [README.md](./README.md) — 产品说明与技术架构
- [AGENTS.md](./AGENTS.md) — 完整 ADR 记录、模块边界、代码约束、关键约束
