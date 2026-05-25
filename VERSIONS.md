# Pandaria Version History

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
| 非测试代码零 `.unwrap()` | ❌（当前 229 个，持续清理中） |

---

### 已知限制

- **CPU time 预算**：tenant 中已预留字段，但无测量和执行逻辑
- **Bedrock 未接入 Router**：`AwsBedrockProvider` 已实现，但 `ProviderFactory`/`ProviderResolver` 缺少对应变体
- **Token 预算预检不完整**：`TenantManagerImpl::send_message()` 中 `check_quota(TokenUsage { input: 0, output: 0 })` 未预估本次请求消耗
- **compaction 未使用 spawn_blocking**：大文本序列化在主线程执行
- **非测试 unwrap**：共 229 个（agent-core: 113, ai-provider: 82, api-gateway: 16, tenant: 3, tui: 15, storage: 0）

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

#### v0.1.4 目标（短期）

- [ ] api-gateway 暴露 `/metrics` endpoint
- [ ] Token 配额预检修复：`TenantManagerImpl::send_message()` 传入实际预估 token
- [ ] 非测试 unwrap 清理：目标降至 50 个以下
- [ ] PRD.md / AGENTS.md / README.md 文档一致性维护

#### v0.2.0 目标（中期）

- [ ] CPU time 预算实现与接入
- [ ] Bedrock provider 接入 Router/Resolver
- [ ] compaction 大文本操作移至 spawn_blocking
- [ ] 水平扩展：session 跨节点迁移能力设计
- [ ] WASM / RPC 插件运行时（重新设计轻量级插件边界）
