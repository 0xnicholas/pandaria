# Pandaria Runtime Openness 规格书

**Date:** 2026-05-20
**Status:** Draft
**Reference:** AGENTS.md (ADR-001, ADR-003, ADR-004, ADR-005)

---

## 1. 模块定位

本规格书定义 Pandaria 从**编译期绑定、工具封闭的 Runtime** 向**可被外部编排器消费的生产级开放 Runtime** 演进的标准。

Pandaria 不做 multi-agent 编排。编排由外部项目（如 Tavern）负责。Pandaria 只提供：
- **可靠的单 agent 执行环境**（隔离、配额、持久化、可观测性）
- **开放的工具配置能力**（外部编排器可为每个 session 指定不同工具集）
- **可查询的 session 状态机**（编排器可感知 agent 是否空闲）
- **多样化的事件投递方式**（SSE / WebSocket / Webhook）
- **便利的 API 原语**（批量操作、同步等待、配额查询）

---

## 2. 依赖方向

```
外部编排器 (Tavern)
  │  HTTP / WebSocket
  ▼
api-gateway ──→ tenant ──→ agent-core ──→ ai-provider
                    │
                    ↓
                 storage
```

**关键约束**：编排器是 Pandaria 的外部消费者，不是内部模块。Pandaria 的依赖树不引入任何编排相关代码。

---

## 3. 需求规格

### 3.1 工具即服务（Tool-as-a-Service）

#### 3.1.1 现状

当前 Pandaria 的工具集在编译期硬编码于 `TenantManagerImpl::create_session()` 中。所有 session 共享同一套工具（仅 `MediaGenerationTool` 是条件注入的）。外部编排器无法通过 API 为不同 agent 配置不同工具。

#### 3.1.2 需求

外部编排器在创建 session 时，可通过 API 声明该 session 可用的工具列表。每个工具指向一个外部 HTTP 端点。Pandaria 在 agent loop 中调用工具时，将 tool_call 转发到对应端点，并把响应返回给 LLM。

#### 3.1.3 核心类型

**`agent-core/src/tools/http_proxy.rs`**（底层定义）：
```rust
pub struct ToolConfig {
    /// 工具名称，作为 tool_call 的标识
    pub name: String,
    /// 工具的人类可读描述，注入 LLM system prompt
    pub description: String,
    /// JSON Schema 描述工具参数
    pub parameters: serde_json::Value,
    /// 工具执行端点（HTTP URL）
    pub endpoint: String,
    /// 请求超时（毫秒），默认 30000
    pub timeout_ms: Option<u64>,
    /// 可选的认证头
    pub headers: Option<HashMap<String, String>>,
}

pub struct HttpProxyTool {
    config: ToolConfig,
    tenant_id: String,
    session_id: String,
    client: reqwest::Client,
}
```

**分层说明与新增文件**：
- `ToolConfig` 定义在 **新增文件** `agent-core/src/tools/http_proxy.rs`，并在此文件的 `mod.rs` 中导出（当前 `agent-core/src/tools/` 仅含 `media_generation.rs`）。
- `HttpProxyTool` 实现 `AgentTool` trait，在构造时绑定 `tenant_id` 和 `session_id`（从 `SessionConfig` 获取），以便在转发请求时注入上下文。不需要修改 `AgentTool` trait 签名。
- **`api-gateway` 不直接复用 `ToolConfig`**（避免 crate 间类型泄漏到 API 契约层）。Gateway 的 `CreateSessionRequest` 独立声明 `tools` 字段，handler 中转换为 `tenant::CreateSessionParams`，再传入 `TenantManager::create_session()`。

**数据流**：
```
CreateSessionRequest.tools  (api-gateway)
  → CreateSessionParams.tools  (tenant)
    → SessionConfig.tools  (agent-core)
      → HttpProxyTool 实例注入 SessionActor
```
因此 `CreateSessionParams` 和 `SessionConfig` 均需新增 `tools: Vec<ToolConfig>` 字段。

#### 3.1.4 行为规格

1. **创建时注入**：`TenantManagerImpl::create_session()` 接收的 `CreateSessionParams` 新增 `tools: Vec<ToolConfig>` 字段（由 `api-gateway` handler 从 `CreateSessionRequest.tools` 转换而来）。为每个 `ToolConfig` 创建一个 `HttpProxyTool` 实例，追加到 `SessionConfig.tools` 中，最终传入 `SessionActor::new()`。
2. **执行时转发**：`HttpProxyTool::execute()` 向 `config.endpoint` 发送 HTTP POST 请求，请求体为 JSON：
   ```json
   {
     "tool_call_id": "call_xxx",
     "params": { ... },
     "session_id": "...",
     "tenant_id": "..."
   }
   ```
3. **响应格式**：外部工具端点应返回：
   ```json
   {
     "content": [{"type": "text", "text": "..."}],
     "details": {},
     "is_error": false,
     "terminate": false
   }
   ```
   `HttpProxyTool` 将该响应映射为 `AgentToolResult`。
   - `terminate` 为 `true` 时，agent loop 在该 tool result 处理后终止（与本地工具的 `terminate` 语义一致）。
4. **超时与取消**：`HttpProxyTool` 使用 `config.timeout_ms` 作为请求超时。若 `CancellationToken` 被取消，应中止正在进行的 HTTP 请求。
5. **错误处理**：若 HTTP 请求失败（非 2xx、超时、网络错误），`HttpProxyTool` 返回 `AgentToolResult { is_error: true, content: [Text { text: "HTTP error: ..." }], ... }`，让 LLM 看到错误信息并决定如何重试。
6. **Hook 兼容性**：`HttpProxyTool` 作为普通 `AgentTool`，自动走 `on_tool_call` 和 `on_tool_result` hook。`DefaultHookDispatcher` 的 `ToolGuard` 和 `Audit` 对其生效。`PathGuard` 仍会扫描参数中的路径字符串，但 `HttpProxyTool` 本身不访问文件系统，因此 PathGuard 的拦截行为不会产生实际副作用。

#### 3.1.5 安全约束

- `config.endpoint` 必须是一个合法的 HTTP/HTTPS URL。
- **SSRF 防护（第一版必须实现）**：`HttpProxyTool::execute()` 在发起请求前检查 endpoint 是否为内网地址或元数据服务：
  - 禁止 `127.0.0.0/8`、`10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`
  - 禁止 `169.254.169.254`（AWS/云厂商元数据服务）
  - 禁止 `localhost` 域名
  - 命中则直接返回 `AgentToolResult { is_error: true, content: [Text { text: "SSRF: internal endpoint forbidden" }] }`
- 未来可通过 `TenantConfig` 增加 **工具端点域名白名单**（正向清单），进一步收紧。
- `config.headers` 中不得包含 `Authorization: Bearer sk-...` 等敏感 token 的明文（建议通过环境变量或 secret 注入）。

---

### 3.2 Session 状态机暴露

#### 3.2.1 现状

`SessionActor` 没有显式状态。外部编排器只能通过 `turn_count` 推断 session 是否活跃，无法判断其是否正在执行 turn。

#### 3.2.2 需求

引入显式的 session 状态机，并通过 REST API 暴露。

#### 3.2.3 状态定义

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    /// Session 已创建，当前无正在执行的 turn
    Idle,
    /// 正在执行 turn（AgentLoop 运行中）
    Running,
    /// 发生了不可恢复的错误，需要人工干预或 reset
    Error,
}
```

#### 3.2.4 状态转换规则

| From | To | Trigger |
|---|---|---|
| Idle | Running | `SessionActor::prompt_with_content()` / `prompt()` / `continue_()` 被调用 |
| Running | Idle | `run_with_messages()` 的全部 loop 迭代成功完成（包括 steer/follow_up 驱动的多次 `AgentLoop::run`） |
| Running | Error | `AgentLoop::run()` 返回不可恢复错误（非 retryable），或 `run_with_messages()` 的 recovery 失败 |
| Error | Idle | `SessionActor::reset()` 被调用 |
| Error | Error | `prompt_with_content()` 被调用（Error 状态下拒绝服务，返回 `AgentError::SessionInError`） |
| Running | Idle | `abort_token.cancel()` 被调用（interrupt） |

#### 3.2.5 API 暴露

```http
GET /api/v1/sessions/{id}/state
Authorization: Bearer <token>

Response 200:
{
  "state": "idle" | "running" | "error",
  "error_reason": null | "context overflow after max retries"
}
```

#### 3.2.6 实现细节

- **`is_streaming` 字段替换为 `state: std::sync::atomic::AtomicU8`**（映射为 `SessionState`），`is_streaming()` 方法改为读取 `state == Running`。原子状态保证查询不持有 actor mutex，避免在 turn 运行期间阻塞外部查询。
- 状态机转换逻辑嵌入 `SessionActor::run_with_messages()` 的入口（`Idle → Running`）和出口（`Running → Idle` 或 `Running → Error`）。`abort_token.cancel()` 也在出口处处理为 `Running → Idle`。
- **新增 `SessionActor::reset()`**：清空 `entries`（消息历史），重置 `recovery` 为初始状态，重置 `state` 为 `Idle`，保留 `system_prompt`、`model`、`tools`、`skills`、`webhook` 等配置。`abort_token` 重新生成（避免旧的 cancel 状态影响新 turn）。
- `Error` 状态下，`prompt_with_content()`、`prompt()`、`continue_()` 立即返回 `AgentError::SessionInError { reason: String }`，必须显式调用 `reset()` 清除错误状态。

---

### 3.3 Webhook 事件推送

#### 3.3.1 现状

仅有 SSE 一种事件投递方式。外部编排器管理 N 个 session 需维持 N 个长连接。

#### 3.3.2 需求

支持 Webhook：当 session 发生关键事件时，Pandaria 主动向编排器注册的 URL 发送 HTTP POST。

#### 3.3.3 核心类型

**`tenant/src/manager.rs`**（与 session 配置同层）：
```rust
pub struct WebhookConfig {
    /// Webhook 接收端点
    pub url: String,
    /// 订阅的事件类型列表，为空则默认订阅 ["turn_end", "error"]
    pub events: Vec<String>,
    /// HMAC 签名密钥（可选）
    pub secret: Option<String>,
}
```

- `WebhookConfig` 放在 `tenant` crate 的 `CreateSessionParams` 中（新增字段 `webhook: Option<WebhookConfig>`），由 `TenantManagerImpl::create_session()` 在创建 `ActiveSession` 时保存到 `ActiveSession` 结构体中。
- 投递组件 `WebhookEventListener` 实现 `agent_core::AgentEventListener`，放在 `tenant/src/events.rs`（与 `SessionEventBridge` 同文件），在 `create_session()` 中通过 `actor.add_event_listener()` 注册到 `SessionActor`。

#### 3.3.4 行为规格

1. **注册时机**：第一版 Webhook 仅在 `CreateSessionRequest` 中配置（`CreateSessionRequest` 新增 `webhook: Option<WebhookConfig>` 字段），创建后不可通过 `UpdateSessionRequest` 动态修改（需要 `SessionActor` 支持运行时增删 event listener，作为后续优化）。
2. **事件过滤**：`WebhookEventListener` 只转发 `config.events` 列表中包含的事件类型。若列表为空，默认订阅 `["turn_end", "error"]`（避免产生过多无效请求）。
3. **请求格式**：
   ```http
   POST <webhook_url>
   Content-Type: application/json
   X-Pandaria-Event: turn_end
   X-Pandaria-Signature: sha256=<hmac>
   X-Pandaria-Delivery: <uuid>
   X-Pandaria-Session-Id: <uuid>
   X-Pandaria-Tenant-Id: <string>
   
   { "type": "turn_end", "turn_index": 5, "stop_reason": "stop", ... }
   ```
   **签名计算**：`HMAC-SHA256(secret, body_bytes)`，其中 `body_bytes` 为请求体的原始 UTF-8 字节。接收方使用相同 secret 对 body 重新计算 HMAC，与 header 中的值比对。
4. **重试与并发策略**：
   - 首次投递失败后，指数退避重试：1s → 2s → 4s，最多 3 次。
   - 若连续失败超过 10 次，自动禁用该 webhook 并记录 error log。
   - 使用内部 `tokio::mpsc` 队列 + 限流执行器实现异步投递，**同一 webhook URL 的最大并发投递数为 5**，防止突发流量压垮接收方。
   - 不阻塞 event processor。
5. **幂等性**：每次投递携带唯一的 `X-Pandaria-Delivery` UUID，接收方可据此去重。
6. **事件类型映射**：Webhook 发送 `ServerEvent`（与 SSE 数据格式一致），而非内部 `AgentEvent`。

#### 3.3.5 新增事件类型

为支持状态机，SSE 和 Webhook 均需新增事件：
```rust
pub enum ServerEvent {
    // ... existing variants ...
    StateChanged { state: String },  // NEW
}
```

---

### 3.4 API 必要补充

#### 3.4.1 租户配额查询

**需求**：编排器在做调度决策前，需要知道当前租户的资源余量。

```http
GET /api/v1/tenant/quota
Authorization: Bearer <token>

Response 200:
{
  "tenant_id": "tenant-1",
  "max_concurrent_sessions": 10,
  "active_sessions": 4,
  "max_tokens_per_day": 1000000,
  "tokens_used_today": 234000,
  "max_tool_calls_per_minute": 60,
  "tool_calls_in_last_minute": 12,
  "default_model": "claude-sonnet-4",
  "available_models": ["claude-sonnet-4", "gpt-4o", "doubao-seed-2.0-pro"]
}
```

**实现**：`TenantManagerImpl` 从 `TenantSupervisor` 读取 meter 数据并组装响应。

#### 3.4.2 同步等待 Turn 完成

**需求**：简化编排器的异步逻辑。对简单场景，编排器希望一次 HTTP 调用就拿到 turn 结果。

```http
POST /api/v1/sessions/{id}/messages?wait=true&timeout_ms=30000
Content-Type: application/json

{ "content": [...] }

Response 200 (若 turn 在超时前完成):
{
  "turn_index": 5,
  "completed": true,
  "messages": [
    { "role": "assistant", "content": [...], ... }
  ],
  "usage": { "input_tokens": 1200, "output_tokens": 450 }
}

Response 202 (若超时未 complete):
{
  "turn_index": 5,
  "completed": false,
  "message": "turn still in progress, subscribe to events for updates"
}
```

**行为规格**：
1. `wait=true` 时，`TenantManagerImpl::send_message()` **先创建临时事件监听器并订阅到 `SessionEventBridge`**。
2. 然后再调用 `prompt_with_content()` 触发 turn。
3. 监听 `AgentEvent::TurnEnd { messages, .. }` 或 `AgentEvent::Error { .. }`。若超时前等到 `TurnEnd`，从 `messages` 中提取最后一个 `AgentMessage::Assistant` 作为响应体中的 `messages`，并组装 `usage`（从 assistant message 的 `usage` 字段或聚合 tool call 结果获得）。
4. 若超时，返回 202，客户端 fallback 到 SSE/Webhook。
5. 最大允许超时由服务端配置限制（默认 60s），防止连接被滥用。
6. **竞态安全**：先订阅、后触发，确保不会错过在订阅前就已发出的事件。

#### 3.4.3 批量创建 Session

**需求**：编排器创建 crew 时，需要一次性创建多个同构 session。

```http
POST /api/v1/sessions/batch
Content-Type: application/json

{
  "count": 3,
  "template": {
    "system_prompt": "You are a research specialist.",
    "model": "gpt-4o",
    "tools": [...],
    "webhook": { "url": "https://tavern.example.com/webhook", "events": ["turn_end"] }
  }
}

Response 201:
{
  "created": [
    { "id": "uuid-1", "title": null, "model": "gpt-4o", ... },
    { "id": "uuid-2", ... },
    { "id": "uuid-3", ... }
  ],
  "failed": []
}
```

**行为规格**：
1. **预检查**：先计算 `active_sessions + count <= max_concurrent_sessions`，不满足则直接返回 429，不创建任何 session。`template` 字段包含 `system_prompt`、`model`、`tools`、`webhook`，均透传到 `CreateSessionParams`。
2. **原子创建**：预检查通过后，逐个调用 `supervisor.reserve_session()` 并创建 session。若中间某一步失败（极罕见），回滚已创建的 session：
   - 从 `TenantManagerImpl::sessions` DashMap 中 remove 已插入的条目
   - 调用 `entry.abort_token.cancel()`
   - `SessionGuard` 随 `ActiveSession` drop 自动释放 slot
   - 返回 500， `failed` 列表包含失败原因
3. 每个创建的 session 使用相同的 `template` 配置，但独立的 session ID 和 abort token。
4. `count` 上限由服务端配置限制（默认 10），防止滥用。

#### 3.4.4 Session 克隆（P1）

**需求**：基于现有 session 快速 spawn 同构 worker。

```http
POST /api/v1/sessions/{id}/clone
Content-Type: application/json

{ "title": "worker-2" }

Response 201: SessionInfo
```

**行为规格**：复制 system_prompt、model、tools、webhook 配置，**不复制消息历史**。

#### 3.4.5 Session 重置（P1）

**需求**：清空历史，保留配置，复用 session ID。

```http
POST /api/v1/sessions/{id}/reset
Response 200: { "state": "idle" }
```

---

## 4. WebSocket 支持（Phase 2）

### 4.1 端点

```
WS /api/v1/sessions/{id}/ws
```

### 4.2 协议

- 认证：WebSocket handshake 时通过 HTTP `Authorization` header 传递 Bearer token（与 REST API 一致）。`api-gateway` 的 `auth_middleware` 应在 WebSocket upgrade 前完成认证。**禁止**通过 query param 传递 token（会 leak 到 server logs）。
- 服务端 → 客户端：JSON 文本帧，格式同 SSE 的 `ServerEvent`。
- 客户端 → 服务端：
  ```json
  { "action": "send_message", "content": [...] }
  { "action": "interrupt" }
  { "action": "ping" }
  ```
- 心跳：服务端每 30s 发送 `{"type": "ping"}`，客户端应回复 `{"type": "pong"}`。

### 4.3 与 SSE 的关系

WebSocket 不是 SSE 的替代，而是补充：
- **SSE**：适合浏览器端简单订阅，HTTP 友好，自动重连。
- **WebSocket**：适合交互式客户端（TUI、Web UI），支持双向通信。
- **Webhook**：适合服务端编排器，无需维持长连接。

---

## 5. 安全与配额

### 5.1 配额影响

| 操作 | 配额消耗 |
|---|---|
| 创建 session | +1 `active_sessions`，受 `max_concurrent_sessions` 限制 |
| 批量创建 N 个 | +N `active_sessions`，整体原子检查 |
| 克隆 session | +1 `active_sessions` |
| 外部工具调用 | 计入 `tool_calls_per_minute` |
| Webhook 投递 | 不计入配额，但受内部速率限制保护 |

### 5.2 安全约束

- `ToolConfig.endpoint` 必须满足 `url::Url` 解析且 scheme 为 `http`/`https`。
- Webhook URL 不得指向内网保留地址（10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 127.0.0.0/8）。
- `X-Pandaria-Signature` 使用 HMAC-SHA256，接收方应验证以防止伪造。
- 同步等待 API (`?wait=true`) 的最大超时由 `ServerConfig.max_sync_wait_ms` 控制，默认 60s。

---

## 6. 错误处理

### 6.1 新增错误码

| Error Code | HTTP Status | 场景 |
|---|---|---|
| `tool_endpoint_invalid` | 400 | `ToolConfig.endpoint` 不是合法 URL |
| `tool_endpoint_forbidden` | 400 | `ToolConfig.endpoint` 命中域名黑名单 |
| `batch_size_exceeded` | 400 | `count` 超过服务端限制 |
| `session_in_error` | 409 | `prompt_with_content()` 在 Error 状态下被调用，必须先 reset。对应 **新增** `AgentError::SessionInError { reason: String }` variant（定义在 `agent-core/src/error.rs`），由 `tenant/src/manager.rs` 的 `map_agent_error()` 映射为 HTTP 409 |
| `webhook_disabled` | 200 (响应体中) | Webhook 连续失败已被自动禁用，不中断 session |
| `webhook_delivery_failed` | 500 (内部) | Webhook 单次投递失败，正在重试 |
| `quota_exceeded` | 429 | 批量创建时整体配额不足 |

### 6.2 Webhook 投递失败处理

Webhook 投递失败**不**影响 agent loop 的执行。它是一个旁路的观测机制。连续失败超过 10 次后，该 webhook 被自动禁用。

**Webhook 健康查询（后续版本）**：
可通过扩展 `GET /api/v1/sessions/{id}` 的响应，增加 `webhook` 字段显示当前 webhook 状态（`active` / `disabled` / `failing`）和最近投递失败次数。

---

## 附录 A：与现有代码的变更摘要

| 变更点 | 当前状态 | 所需修改 |
|---|---|---|
| `CreateSessionRequest` | `api-gateway/src/types.rs`，仅 `title` + `system_prompt` | 新增 `tools: Option<Vec<ToolConfig>>`、`webhook: Option<WebhookConfig>` |
| `CreateSessionParams` | `tenant/src/manager.rs`，仅 `title` + `system_prompt` | 新增 `tools: Vec<ToolConfig>`、`webhook: Option<WebhookConfig>` |
| `SessionConfig` | `agent-core/src/harness/session.rs`，无 tools/webhook | 新增 `tools` 已存在，无需变更；webhook 由 tenant 层监听，不传入 SessionActor |
| `agent-core/src/tools/` | 仅 `media_generation.rs` | **新增** `http_proxy.rs`（`ToolConfig` + `HttpProxyTool`），`mod.rs` 导出 |
| `SessionActor` | 有 `is_streaming: bool`，无 `reset()` | `is_streaming` 替换为 `state: AtomicU8`；**新增** `reset()` 方法 |
| `ActiveSession` | `tenant/src/session_entry.rs` | 新增 `webhook: Option<WebhookConfig>`，用于 WebhookEventListener 生命周期管理 |
| `AgentError` | `agent-core/src/error.rs` | **新增** `SessionInError { reason: String }` variant |
| `ServerEvent` | `api-gateway/src/types.rs` | 新增 `StateChanged { state: String }` variant |

## 7. 实施阶段

| Phase | 范围 | 交付物 |
|---|---|---|
| **Phase 1** | `HttpProxyTool` + 动态工具注册 | `agent-core/src/tools/http_proxy.rs`, `tenant/src/manager.rs`（解析 ToolConfig 并注入）, `api-gateway/src/types.rs` |
| **Phase 2** | Session 状态机 + `/state` 端点 | `SessionState` enum, `GET /sessions/{id}/state` |
| **Phase 3** | Webhook 事件推送 | `WebhookEventListener`, `X-Pandaria-Signature`, 重试队列 |
| **Phase 4** | API 必要补充 | `/tenant/quota`, `/messages?wait=true`, `/sessions/batch`, `/sessions/{id}/clone`, `/sessions/{id}/reset` |
| **Phase 5** | WebSocket 端点 | `WS /sessions/{id}/ws`, 心跳, 双向消息 |
| **Phase 6** | OpenAPI 文档 + E2E 测试 | `docs/openapi.yaml` 更新, integration tests |
