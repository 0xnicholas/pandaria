# Pandaria Version History

---

## v0.2.0 — 2026-05-27

### 状态

v0.2.0 完成持久化加固、Memory 数据准备重构、E2E 测试矩阵扩展三项核心工作。

### 变更摘要

#### 持久化加固

- **自动 restore**：`SessionActor` 在首次 `prompt()` 时自动从 store 恢复历史，无需调用方手动调 `restore()`。原 `restore()` 方法标记 deprecated。
- **增量保存**：`SessionStore` trait 新增 `append_entries()` 默认方法（load→merge→save）。PG 适配器 override 为 `jsonb || jsonb` 串联，避免全量序列化。
- **api-gateway 持久化接入**：`HarnessConfig.store` 可通过 HTTP API 注入，session 重启后可恢复。
- **MemoryStore forget 联动**：`TenantManagerImpl::delete_session()` 同步调用 `MemoryStore::forget_session()`。
- **`EmeraldMemoryStore` HTTP adapter**：新增 `agent-core/src/memory/emerald.rs`，实现 `MemoryStore` trait → Emerald REST API（`remember`/`recall`/`forget_session`），含 7 个 mock-based 单元测试。

#### Memory 数据准备重构

- **MemoryStore trait 简化**：`remember(ctx, facts)` → `remember(ctx, content, metadata)`。删除 `MemoryFact`、`MemoryQuery` 类型。
- **Conversation Formatter**：新增 `memory/formatter.rs`，将 agent turn 格式化为 Markdown 文本 + 结构化元数据，供 Emerald 等外挂记忆系统消费。
- **MemoryHookDispatcher 重写**：`on_turn_end` 调用 formatter 生成 Markdown 内容，fire-and-forget 发送到外挂 store。
- **MemoryContext 增强**：新增 `model` 和 `session_started_at` 字段。

#### E2E 测试矩阵

- **新增 5 个 E2E 测试文件**：
  - `e2e_persistence_recovery` — 全链路持久化恢复（store 级别验证）
  - `e2e_persistence_compaction` — compaction + persistence 组合
  - `e2e_persistence_fault_injection` — DB 不可用降级
  - `e2e_concurrent_sessions` — 并发 session 隔离
  - `e2e_memory_store` — MemoryStore 联动（格式验证 + API 集成）
- **E2E Coverage Gap（4 个新测试）**：
  - `e2e_path_guard` — PathGuard 拦截越界文件访问
  - `e2e_media_provider` — MediaGenerationTool 内联/文件保存双路径
  - `e2e_rate_limit` — 令牌桶限流（burst、per-tenant 隔离、refill）
  - `e2e_token_budget` — max_turns 非阻断警告验证
- **测试基础设施**：`common.rs` 新增 `build_test_app_with_store`、`build_test_app_with_store_and_compaction`、`ensure_test_containers`、`create_test_pg_store` 等工厂函数。

#### Emerald 集成

- **Spec**: `docs/specs/2026-05-27-pandaria-emerald-memorystore.md`
- **实现**: `crates/agent-core/src/memory/emerald.rs`
- **配置项**: `EMERALD_BASE_URL`、`EMERALD_API_KEY`

#### 文档

- Spec: `docs/specs/2026-05-26-persistence-e2e-hardening.md`
- Plan: `docs/plans/2026-05-26-persistence-e2e-hardening.md`
- Spec: `docs/specs/2026-05-27-pandaria-emerald-memorystore.md`
- Plan: `docs/superpowers/plans/2026-05-27-e2e-coverage-gap.md`

#### Pawbun 工具生态（2026-06）

- **pawbun-files**：多模态文件处理层，统一 `FileLoader` + `ProviderFormat`（OpenAI/Anthropic/Gemini/Azure）
- **pawbun-toolkit**：Agent 工具抽象（`Tool` trait、`ToolKit` registry、`AsyncTool`、MCP client adapter）
- **pawbun-toolkit-macros**：`pawbun-toolkit` 过程宏
- **pawbun-mcp-server**：MCP Server（stdio + SSE transport），暴露 Pawbun 工具为 MCP 协议
- **agent-core 接入**：`agent-core` 通过 Cargo dependency 接入 `pawbun-toolkit`

#### Tavern 工作流引擎（2026-06）

- **tavern-core**：工作流/Agent 组合核心类型（`AgentConfig`、`Plan`、`ToolRegistry`）
- **tavern-comp**：编排引擎（`WorkflowEngine`、`StepExecutor`、`FlowStepExecutor`、`EventStore` + PG/SQLite/Memory、`replay`、DAG 校验、Webhook、Timer）
- **tavern-flow-macros**：工作流 DSL 过程宏
- **api-gateway 接入**：`api-gateway` 通过 Cargo dependency 接入 `tavern-comp`

#### Circuit Breaker（2026-06）

- **`agent-core/src/circuit_breaker.rs`**：LLM provider 调用熔断器，Closed→Open→HalfOpen 状态机

#### Hook 系统增强（2026-06）

- **`CombinedDispatcher`**（`agent-core/src/hook/combined.rs`）：多 `HookDispatcher` 链式组合
- **`with_timeout()`**（`agent-core/src/hook/timeout.rs`）：Hook 超时保护，panic 捕获 + 超时兜底

---

## v0.1.4 — 2026-05-25

### 状态

v0.1.4 在 v0.1.3 基础上完成三项基础设施补强：`/metrics` endpoint、token 配额预检修复、unwrap 债务澄清。

---

### 变更摘要

- **api-gateway**: 新增 `/metrics` 路由，返回 prometheus 格式指标（当前仅 `pandaria_active_sessions` gauge）
- **tenant**: `TenantManagerImpl::send_message()` 传入基于消息内容字符数的 token 估算（`chars / 4` 启发式）
- **tenant**: 3 个生产代码 `.unwrap()`（`abort_token.lock()`）替换为 `.expect("abort_token lock poisoned")`
- **文档**: 确认所有 229 个 `.unwrap()` 均位于 `mod tests` 块内，生产代码实际为 0

---

## v0.1.3 — 2026-05-25

### 状态

服务端多租户 agent runtime 核心架构已落地，6 个 crate 全部可用，API Gateway + TUI 客户端可端到端运行。本版本主要解决文档债务和版本号对齐。

---

### 包含内容

#### 核心运行时

- **ai-provider** — LLM provider 抽象层，支持 Anthropic、OpenAI、Google、Mistral、DeepSeek，AWS Bedrock feature-gated 占位
- **agent-core** — Agent Loop（原生 Tool Use 协议，并行 ToolCall）、SessionActor（生命周期 + 持久化）、CompactionActor、RecoveryStateMachine、PromptBuilder、Skills 注入、DefaultHookDispatcher（内联策略）
- **storage** — Session 持久化层，PostgreSQL + Redis 双适配器

#### 多租户与接入

- **tenant** — 租户注册表、并发配额（RAII SessionGuard）、token/tool call 滑动窗口计量、Session 生命周期管理
- **api-gateway** — REST API + SSE 事件流、HMAC-SHA256 Bearer Token 认证、per-tenant 令牌桶限流
- **tui** — ratatui 终端客户端，REST client + SSE 订阅，完整 keybindings 和命令面板

#### 多模态

- **理解型多模态** — Image/Video/Audio 输入（`Content` enum 支持）
- **生成型多模态** — `MediaProvider` trait + `MediaGenerationTool`

#### 基础设施

- **AgentSpace 统一目录** — `~/.pandaria/` 根目录，含 config/cache/logs/temp/skills/workspaces
- **sanitize** — 敏感数据脱敏（原 observability crate 已删除并内联至 agent-core）

---

### 架构变更（v0.1.x）

| 变更项 | 说明 |
|---|---|
| `extensions` crate 删除 | 原 `Extension` trait、`ExtensionActor`、`HookRouter`、`EventBus` 全部移除 |
| Hook 机制内联 | 内置策略（audit、path_guard、tool_guard、token_budget）内联至 `DefaultHookDispatcher` |
| Hook 调用方式 | 直接函数调用（无 Actor、无 EventBus、无 500ms/100ms 超时边界） |
| panic 处理 | 由 `AgentLoop`/`ToolExecutor` 统一捕获，不传播到其他 session |

---

### 关键约束达成

| 约束 | 状态 |
|---|---|
| 所有异步代码使用 tokio | ✅ |
| 跨 crate 错误类型使用 thiserror | ✅ |
| LLM API 指数退避重试（最多 3 次） | ✅ |
| `tenant_id` 出现在所有 tracing 日志 | ✅ |
| Hook 路径校验（path_guard） | ✅ |
| LLM API Key 不在日志/错误中泄漏 | ✅ |
| 集成测试使用 testcontainers（PG + Redis） | ✅ |
| 所有 crate 含 README.md | ✅ |
| 非测试代码零 `.unwrap()` | ✅（经核查，229 个全部位于 `mod tests` 块内） |

---

### 已知限制

- **CPU time 预算**：tenant 中已预留字段，但无测量和执行逻辑
- **Bedrock 未接入 Router**：`AwsBedrockProvider` 已实现，但 `ProviderFactory`/`ProviderResolver` 缺少对应变体
- **Token 预算预检不完整**：`TenantManagerImpl::send_message()` 中 `check_quota(TokenUsage { input: 0, output: 0 })` 未预估本次请求消耗
- **compaction 未使用 spawn_blocking**：大文本序列化在主线程执行
- **tavern 持续迭代中**：工作流引擎核心已实现，但仍在完善中（Webhook、Timer、错误恢复等细节）

---

### 依赖版本

- Rust Edition 2024
- tokio 1.x
- axum 0.8（api-gateway）
- sqlx 0.8（PostgreSQL）
- redis 0.27
- testcontainers 0.27 + testcontainers-modules 0.15

---

### 升级路径

#### v0.1.4 目标（短期）— 2026-05-25

- [x] api-gateway 暴露 `/metrics` endpoint — 返回 prometheus 格式 `pandaria_active_sessions`
- [x] Token 配额预检修复：`TenantManagerImpl::send_message()` 使用 `estimate_input_tokens(content)`（chars / 4 启发式）
- [x] 非测试 unwrap 清理 — 经逐文件行号核查，229 个全部位于 `mod tests` 块内；tenant 生产代码 3 个已修复
- [x] PRD.md / AGENTS.md / README.md 文档一致性维护 — v0.1.3 已完成

#### v0.2.0 目标（中期）

- [x] Pawbun 工具生态（pawbun-files、pawbun-toolkit、pawbun-mcp-server）
- [x] Tavern 工作流引擎（tavern-core、tavern-comp、EventStore、replay、DAG 校验）
- [x] Circuit Breaker（LLM 调用熔断器）
- [x] Hook 系统增强（CombinedDispatcher + with_timeout）
- [ ] CPU time 预算实现与接入
- [ ] Bedrock provider 接入 Router/Resolver
- [ ] compaction 大文本操作移至 spawn_blocking
- [ ] 水平扩展：session 跨节点迁移能力设计
- [ ] WASM / RPC 插件运行时（重新设计轻量级插件边界）
