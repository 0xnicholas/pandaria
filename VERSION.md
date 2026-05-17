# Pandaria Version

## v0.1.2 — 2026-05-16

### 状态

服务端多租户 agent runtime 核心架构已落地，8 个 crate 全部可用，API Gateway + TUI 客户端可端到端运行。

---

### 包含内容

#### 核心运行时

- **ai-provider** — LLM provider 抽象层，支持 Anthropic、OpenAI、Google、Mistral、DeepSeek，Bedrock feature-gated
- **agent-core** — Agent Loop（原生 Tool Use 协议，并行 ToolCall）、SessionActor（生命周期 + 持久化）、CompactionActor、Circuit Breaker、PromptBuilder、Skills 注入
- **extensions** — Extension 系统（14+ hook 点），Actor Mailbox + EventBus 混合模型，7 个内置扩展（audit、content_filter、path_guard、rate_limit、token_budget、tool_guard）
- **storage** — Session 持久化层，PostgreSQL + Redis 双适配器

#### 多租户与接入

- **tenant** — 租户注册表、并发配额（RAII SessionGuard）、token/tool call 滑动窗口计量、Session 生命周期管理
- **observability** — tracing 初始化、Prometheus metrics 导出、敏感数据脱敏（sanitize）
- **api-gateway** — REST API + SSE 事件流、HMAC-SHA256 Bearer Token 认证、per-tenant 令牌桶限流
- **tui** — ratatui 终端客户端，REST client + SSE 订阅，完整 keybindings 和命令面板

---

### 关键约束达成

| 约束 | 状态 |
|---|---|
| 所有异步代码使用 tokio | ✅ |
| Hook 超时（500ms 阻断/链式，100ms 观测型） | ✅ |
| 跨 crate 错误类型使用 thiserror | ✅ |
| 非测试代码零 `.unwrap()` | ✅ |
| LLM API 指数退避重试（最多 3 次） | ✅ |
| `tenant_id` 出现在所有 tracing 日志 | ✅ |
| Extension 路径校验（path_guard） | ✅ |
| LLM API Key 不在日志/错误中泄漏 | ✅ |
| 集成测试使用 testcontainers（PG + Redis） | ✅ |
| 所有 crate 含 README.md | ✅ |

---

### 已知限制

- **CPU time 预算**：tenant 中已预留字段，但无测量和执行逻辑
- **Bedrock 未接入 Router**：`AwsBedrockProvider` 已实现，但 `ProviderFactory`/`ProviderResolver` 缺少对应变体
- **observability 未深度集成**：metrics 记录函数（`record_tool_call`、`record_llm_tokens` 等）存在，但 agent-core/tenant/api-gateway 尚未调用
- **Token 预算预检不完整**：`TenantManagerImpl::send_message()` 中 `check_quota(TokenUsage { input: 0, output: 0 })` 未预估本次请求消耗
- **ADR-004 Actor 模型偏差**：AgentLoop 和 CompactionActor 是 SessionActor 内部同步调用组件，非独立 tokio actor（ExtensionActor/EventProcessor 符合 Actor 模型）
- **compaction 未使用 spawn_blocking**：大文本序列化在主线程执行

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

#### v0.1.3 目标（短期）

- [ ] observability 与 agent-core/tenant/api-gateway 深度集成（调用 `record_*` 埋点）
- [ ] Bedrock 接入 RouterProvider / ProviderResolver
- [ ] tenant CPU time 预算测量与执行

#### v0.2.0 目标（中期）

- [ ] WebSocket 接入层（补充现有 SSE）
- [ ] Session 跨节点迁移支持
- [ ] per-tenant 分布式 tracing（OpenTelemetry）
