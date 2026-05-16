# api-gateway 详细模块规格

**Date:** 2026-05-02
**Status:** Draft
**Reference:** AGENTS.md (依赖方向, ADR-005), TUI spec (客户端 API 协议)

---

## 模块定位

服务端入口层。为外部客户端（TUI、未来 SDK）提供 HTTP REST + SSE 接入点。职责：认证、路由、SSE 事件转发、限流。

**不负责**：session 生命周期管理、agent loop 执行、租户调度——这些由 tenant / agent-core 负责。

## 依赖方向

```
api-gateway → tenant → extensions → agent-core → ai-provider
```

Gateway 通过 `TenantManager` trait（由 tenant crate 实现）与下游交互，禁止反向依赖。

---

## 1. 文件结构

```
crates/api-gateway/
  Cargo.toml
  README.md
  src/
    lib.rs                     # re-exports, server bootstrap fn
    server.rs                  # axum Router assembly, graceful shutdown
    config.rs                  # ServerConfig (bind addr, auth secret, rate limits)
    types.rs                   # ServerEvent enum, SessionInfo, ApiError
    error.rs                   # GatewayError, HTTP status mapping → IntoResponse
    sse.rs                     # SseStream: axum IntoResponse, mpsc→SSE forwarding
    routes/
      mod.rs
      sessions.rs              # CRUD: POST create, GET list, GET metadata + system prompt
      messages.rs              # POST send message, DELETE interrupt current turn
      events.rs                # GET SSE event stream per session
      health.rs                # GET /healthz
    middleware/
      mod.rs
      auth.rs                  # Bearer token → tenant_id extraction (HMAC)
      rate_limit.rs            # per-tenant token bucket
      tracing_mw.rs            # tracing span injection (tenant_id, session_id)
  tests/
    auth_tests.rs              # Token validation, rejection cases
    routes_tests.rs            # Endpoint integration with mock TenantManager
    sse_tests.rs               # SSE stream send/close/disconnect
```

---

## 2. 依赖

```toml
[dependencies]
axum = "0.8"
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "trace"] }
tokio = { workspace = true, features = ["sync", "rt", "macros", "signal"] }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
futures = { workspace = true }
uuid = { version = "1", features = ["v4"] }
hmac = "0.12"
sha2 = "0.10"
secrecy = { version = "0.8", features = ["serde"] }
dashmap = "6"
tenant = { path = "../tenant" }
agent-core = { path = "../agent-core" }

[dev-dependencies]
tokio-test = "0.4"
tower = { version = "0.5", features = ["util"] }
```

> **依赖约束**：api-gateway **禁止**直接依赖 `ai-provider`（`llm_client`）。所有与 LLM 相关的类型（如 `Usage`）必须在 gateway 内部重新定义，以遵守 `api-gateway → tenant → extensions → agent-core → ai-provider` 的单向依赖方向。
>
> `agent-core` 作为直接依赖是允许的（方向 `api-gateway → agent-core` 不形成环），用于访问 `AgentEvent` 以完成 `AgentEvent → ServerEvent` 映射。tenant crate 已 re-export `AgentEvent`，gateway 也可通过 `tenant::AgentEvent` 使用。

---

## 3. TenantManager trait（依赖反转边界）

Gateway 通过此 trait 与 tenant/session 层交互。定义在 tenant crate 中，此处列为 Gateway 视角的依赖接口。

```rust
use async_trait::async_trait;
use agent_core::events::AgentEvent;
use uuid::Uuid;

/// 由 tenant crate 实现，Gateway 通过 Arc<dyn TenantManager> 注入。
/// AgentEvent 可通过 `tenant::AgentEvent` 或 `agent_core::AgentEvent` 访问。
#[async_trait]
pub trait TenantManager: Send + Sync {
    /// 创建新 session
    async fn create_session(
        &self,
        tenant_id: &str,
        params: CreateSessionParams,
    ) -> Result<SessionInfo, TenantError>;

    /// 列出 tenant 下所有 session
    async fn list_sessions(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<SessionInfo>, TenantError>;

    /// 获取单个 session 元数据
    async fn get_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<SessionInfo, TenantError>;

    /// 发送用户消息，触发新的 agent turn
    async fn send_message(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        content: String,
    ) -> Result<u64, TenantError>;

    /// 中断当前 in-flight turn
    async fn interrupt(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<(), TenantError>;

    /// 订阅 AgentEvent 流，每个 SSE 连接独立订阅。
    /// Drop receiver 即取消订阅。
    async fn subscribe_events(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<tokio::sync::mpsc::Receiver<AgentEvent>, TenantError>;

    /// 删除 session 并释放所有关联资源。
    async fn delete_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<(), TenantError>;

    /// 更新 session 元数据（部分更新）。
    async fn update_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        updates: SessionUpdates,
    ) -> Result<SessionInfo, TenantError>;

    /// 触发 session 上下文压缩。
    async fn compact_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<(), TenantError>;

    /// 获取 session 完整消息历史。
    async fn get_session_messages(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<Vec<agent_core::types::AgentMessage>, TenantError>;

    /// 优雅关闭所有 session。
    async fn shutdown(&self);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionParams {
    /// TUI 客户端发送的 session 标题。
    pub title: Option<String>,
    pub system_prompt: Option<String>,
}

/// Session 部分更新参数。所有字段均为可选，缺失表示不修改。
#[derive(Debug, Clone, Default)]
pub struct SessionUpdates {
    pub title: Option<Option<String>>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
}

/// Session 元数据。此定义与 tenant crate 实际返回的 SessionInfo 对齐。
/// ⚠️ 注意：与 TUI `client/model.rs` 的 SessionInfo 字段存在差异，gateway 需做转换。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: Uuid,
    pub tenant_id: String,
    pub created_at: String,
    pub turn_count: u64,
    pub system_prompt: Option<String>,
    pub title: Option<String>,
    /// tenant crate 已返回此字段，gateway 直接转发。
    pub model: String,
    /// tenant crate 未返回此字段，gateway 从 `ServerConfig.default_context_window` 填充。
    pub context_window: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum TenantError {
    #[error("tenant not found: {0}")]
    TenantNotFound(String),
    #[error("tenant already registered: {0}")]
    TenantAlreadyExists(String),
    #[error("session limit exceeded for tenant {tenant_id}: max {max}, current {current}")]
    SessionLimitExceeded { tenant_id: String, max: u32, current: u32 },
    #[error("token budget exceeded for tenant {tenant_id}: consumed {consumed}, budget {budget}")]
    TokenBudgetExceeded { tenant_id: String, consumed: u64, budget: u64 },
    #[error("tool call rate limit exceeded for tenant {tenant_id}: {calls} calls in window")]
    ToolCallRateLimitExceeded { tenant_id: String, calls: usize },
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// 此 SessionInfo 定义与 tenant crate 实际返回的类型对齐。
/// 由于与 TUI `client/model.rs` 的 SessionInfo 字段存在差异，gateway 负责字段转换与填充默认值。
```

关键设计点：
- `send_message` 返回 `u64`（turn_index），不等待 turn 完成。SSE 连接会收到后续事件。
- `subscribe_events` 每个 SSE 连接独立调用，返回私有的 `mpsc::Receiver`，断开即取消。
- `AgentEvent` 来自 `agent-core`，Gateway 只做映射转发，不解析事件语义。
- `TenantError` 映射为 HTTP 404 / 429 / 500。

**实现依赖排序：** `TenantManager` trait 及其关联类型正式定义在 `crates/tenant/` 中（已实现）。Gateway 的 `routes` 层可先通过 mock `TenantManager` 开发测试。生产就绪依赖 `crates/tenant/` 完成。

---

## 4. API 端点

### 4.1 端点表

| 方法 | 路径 | Handler | 认证 |
|---|---|---|---|
| `GET` | `/healthz` | `health::get` | 无 |
| `POST` | `/api/v1/sessions` | `sessions::create` | Bearer |
| `GET` | `/api/v1/sessions` | `sessions::list` | Bearer |
| `GET` | `/api/v1/sessions/{id}` | `sessions::get` | Bearer |
| `PATCH` | `/api/v1/sessions/{id}` | `sessions::update` | Bearer | 部分更新 session 元数据 |
| `DELETE` | `/api/v1/sessions/{id}` | `sessions::delete` | Bearer |
| `POST` | `/api/v1/sessions/{id}/messages` | `messages::send` | Bearer |
| `DELETE` | `/api/v1/sessions/{id}/messages/current` | `messages::interrupt` | Bearer |
| `GET` | `/api/v1/sessions/{id}/events` | `events::stream` | Bearer |
| `POST` | `/api/v1/sessions/{id}/compact` | `sessions::compact` | Bearer | 触发上下文压缩 |
| `GET` | `/api/v1/sessions/{id}/messages` | `sessions::messages` | Bearer | 获取历史消息列表 |

### 4.2 Request/Response

**POST /api/v1/sessions**
```
Request:  { "title": "..." | null, "system_prompt": "..." | null }
Response: 201 { "id": "uuid", "tenant_id": "...", "created_at": "...",
                "turn_count": 0, "system_prompt": "...",
                "title": "...", "model": "...", "context_window": 128000 }
```
> **Serde 兼容：** TUI 的 `CreateSessionRequest` 当前仅包含 `title` 字段（无 `system_prompt`）。Gateway 的反序列化器必须支持缺失字段：`system_prompt` 需标记 `#[serde(default)]` 或定义为 `Option<String>`，确保 TUI 发送 `{ "title": null }` 时不会报错。
> 注意：`model` 由 tenant crate 返回，gateway 直接转发；`context_window` 由 gateway 从 `ServerConfig` 填充默认值；`title` 由 tenant crate 返回。

**GET /api/v1/sessions**
```
Response: 200 [ SessionInfo, ... ]
```

**GET /api/v1/sessions/{id}**
```
Response: 200 SessionInfo | 404
```
> 返回的 `SessionInfo` 中 `model` 由 tenant crate 直接返回，`context_window` 由 gateway 从 `ServerConfig` 填充默认值。

**PATCH /api/v1/sessions/{id}**
```
Request:  { "model": "claude-sonnet-4", "title": "new title", "system_prompt": "..." }
Response: 200 SessionInfo | 404
```
> 部分更新：只更新请求体中提供的字段。`model` / `system_prompt` 变更会影响后续 turn 的 LLM 调用。`title` 仅影响展示。
> 反序列化器需支持缺失字段：所有字段均为可选，使用 `#[serde(default)]` 或 `Option<T>`。

**DELETE /api/v1/sessions/{id}**
```
Response: 204 | 404
```

**POST /api/v1/sessions/{id}/messages**
```
Request:  { "content": "user message text" }
Response: 202 { "turn_index": 1 }
```

**DELETE /api/v1/sessions/{id}/messages/current**
```
Response: 204 (no body) | 404
```

**GET /api/v1/sessions/{id}/events** → SSE `text/event-stream`

**POST /api/v1/sessions/{id}/compact**
```
Response: 202 Accepted | 404
```
> 触发 session 的上下文压缩（compaction）。异步执行，客户端通过 SSE 收到 `CompactionStart` / `CompactionEnd` 事件。
> ⚠️ 当前 `AgentEvent` 包含 compaction 事件，但 gateway 的 `ServerEvent` 未映射这些事件（MVP 不转发 compaction 事件到客户端）。

**GET /api/v1/sessions/{id}/messages**
```
Response: 200 [ Message, ... ] | 404
```
> 返回 session 的完整消息历史（不包含 compaction entry）。Message 结构与 agent-core 的 `AgentMessage` 一致（`user` / `assistant` / `toolResult`）。
> TUI 可用于：
> - 切换 session 时恢复历史消息
> - 断线重连后恢复上下文

安全约束：认证失败、资源不存在统一返回 404，不泄露资源存在性。只有 healthz 返回 200 无认证。

---

## 5. SSE 事件协议

### 5.1 ServerEvent 类型

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ServerEvent {
    /// 新消息开始（客户端可据此初始化消息容器）
    #[serde(rename = "message_start")]
    MessageStart { message_index: u64 },

    /// 流式文本增量
    #[serde(rename = "text_delta")]
    TextDelta { delta: String },

    /// thinking / reasoning 内容增量
    /// ⚠️ 预留：当前 agent-core 未生成对应 AgentEvent，MVP 阶段不会触发。
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { content_index: usize, delta: String },

    /// LLM 开始调用工具
    /// ⚠️ 预留：当前 agent-core 未生成对应 AgentEvent，MVP 阶段不会触发。
    #[serde(rename = "tool_call_started")]
    ToolCallStarted { call_id: String, name: String },

    /// 工具参数流式增量
    /// ⚠️ 预留：当前 agent-core 未生成对应 AgentEvent，MVP 阶段不会触发。
    #[serde(rename = "tool_call_delta")]
    ToolCallDelta { call_id: String, delta: String },

    /// 工具执行完成（含结果）
    #[serde(rename = "tool_call_done")]
    ToolCallDone { call_id: String, result: Option<String>, #[serde(default)] is_error: bool },

    /// 当前 turn 结束
    #[serde(rename = "turn_end")]
    TurnEnd { stop_reason: String, usage: Option<UsageInfo> },

    /// 错误事件
    #[serde(rename = "error")]
    Error { code: String, message: String },
}

/// Token 使用量统计。api-gateway 独立定义，不依赖 ai-provider 的 Usage 类型。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageInfo {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}
```

### 5.2 AgentEvent → ServerEvent 映射

agent-core 定义的 `AgentEvent` 变体（`crates/agent-core/src/events.rs`）：

```rust
pub enum AgentEvent {
    AgentStart,
    AgentEnd { messages: Vec<AgentMessage> },
    TurnStart { turn_index: u64 },
    TurnEnd { turn_index: u64, messages: Vec<AgentMessage> },
    MessageStart { message_index: u64 },
    MessageUpdate { message_index: u64, content_delta: String },
    MessageEnd { message: AgentMessage },
    ToolExecutionStart { tool_call_id: String, tool_name: String },
    ToolExecutionUpdate { tool_call_id: String, content: String },
    ToolExecutionEnd { tool_call_id: String, result: ToolResultMessage },
    CompactionStart { reason: CompactReason },
    CompactionEnd { reason: CompactReason, result: Option<CompactionResult>, aborted: bool, will_retry: bool, error_message: Option<String> },
    AutoRetryStart { attempt: u32, max_attempts: u32, delay_ms: u64 },
    AutoRetryEnd { success: bool, error: Option<String> },
    Error { error: AgentError },
}
```

| AgentEvent | ServerEvent | 映射逻辑 |
|---|---|---|
| `MessageStart { message_index }` | `MessageStart { message_index }` | 直通。通知客户端新的 assistant message 开始。 |
| `MessageUpdate { message_index, content_delta }` | `TextDelta { delta }` | `delta = content_delta`，直通 |
| `ToolExecutionStart { tool_call_id, tool_name }` | — | 不发送。工具执行与 tool call 参数累积是不同概念，客户端只关心参数和最终结果。 |
| `ToolExecutionUpdate { tool_call_id, content }` | — | 不发送。MVP 不支持流式工具输出。 |
| `ToolExecutionEnd { tool_call_id, result }` | `ToolCallDone { call_id, result, is_error }` | `call_id = tool_call_id`。`result` 从 `ToolResultMessage.content` (`Vec<Content>`) 提取：拼接所有 `Content::Text` 的 `text` 字段为 `String`；若无 Text 内容则为 `None`。`is_error = result.is_error`。 |
| `TurnEnd { turn_index, messages }` | `TurnEnd { stop_reason, usage }` | `stop_reason` 和 `usage` 从 `messages` 中**最后一个 `AssistantMessage`** 提取：`stop_reason = assistant.stop_reason.to_string()`, `usage = UsageInfo { input_tokens, output_tokens }`。`usage` 可能为 `None`（如工具调用 turn 无 LLM 调用）。 |
| `AgentEnd { messages }` | — | 不发送。客户端通过 `TurnEnd` 感知每轮结束，无需显式的 agent 结束事件。 |
| `Error { error }` | `Error { code, message }` | `code = error.variant_name()`, `message = error.to_string()` |
| `CompactionStart` / `CompactionEnd` | — | **MVP 不转发**。客户端通过 `TurnEnd` 和后续消息变化感知上下文变更，无需显式 compaction 事件。 |
| `AutoRetryStart` / `AutoRetryEnd` | — | **MVP 不转发**。自动重试对客户端透明，客户端继续接收正常的 `MessageUpdate` / `TurnEnd` 流。 |
| `AgentStart` / `TurnStart` / `MessageEnd` | — | 不发送 |

**协议预留字段**（当前 `AgentEvent` enum 中不存在，但 `ServerEvent` 已预留）：

| ServerEvent 预留字段 | 说明 |
|---|---|
| `ThinkingDelta { content_index, delta }` | 未来若 agent-core 扩展支持 reasoning/thinking 流式输出则启用。当前 agent loop 对 `ai_provider::AssistantMessageEvent::ThinkingDelta` 做空处理 `{}`。 |
| `ToolCallStarted { call_id, name }` | 未来若 agent-core 扩展支持 tool call 参数流式累积则启用。当前 `ToolExecutionStart` 不映射为此事件（两者语义不同）。 |
| `ToolCallDelta { call_id, delta }` | 同上，tool call 参数增量预留。 |

### 5.3 SseStream（sse.rs）

```rust
use axum::response::{sse::Event, IntoResponse, Sse};
use futures::stream::Stream;
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

/// 将 mpsc::Receiver<ServerEvent> 转为 axum SSE 响应。
pub struct SseStream {
    rx: tokio::sync::mpsc::Receiver<ServerEvent>,
}

impl SseStream {
    pub fn new(rx: tokio::sync::mpsc::Receiver<ServerEvent>) -> Self {
        Self { rx }
    }
}

impl IntoResponse for SseStream {
    fn into_response(self) -> axum::response::Response {
        Sse::new(EventStream { rx: self.rx })
            .keep_alive(
                axum::response::sse::KeepAlive::new()
                    .interval(std::time::Duration::from_secs(15))
                    .text("ping"),
            )
            .into_response()
    }
}

// impl Stream<Item = Result<Event, Infallible>> for EventStream
// 每条 ServerEvent 序列化为:
//   event: <type in snake_case>
//   data: <json>
//   <blank line>
// receiver closed → stream ends → client disconnect
```

### 5.4 events.rs 路由处理逻辑

```
GET /api/v1/sessions/{id}/events:
  1. 从 request extensions 提取 tenant_id
  2. 解析 session_id 为 Uuid
  3. 调用 tenant_manager.subscribe_events(tenant_id, session_id)
  4. spawn tokio task:
       loop {
         recv AgentEvent from rx:
           map to ServerEvent (见映射表)
           send ServerEvent to sse_tx
       }
       若 recv 返回 None（channel closed）:
         drop sse_tx → SSE stream 结束
  5. 返回 SseStream(sse_rx) 作为 SSE 响应体
  6. 客户端断开 TCP → sse_rx dropped → task 自动终止 → AgentEvent receiver dropped
     → tenant 检测 subscriber gone → 清理订阅
```

> **TUI 对齐：** `SererEvent` 是本 spec 的权威定义。TUI 的 `client/model.rs` 必须定义与上述 enum 所有变体字段匹配的 deserialization struct，包括 `TextDelta.message_index`、`TurnEnd.turn_index` 等字段，即使 TUI spec 的 handler 示例未列出这些字段。

---

## 6. 认证中间件

### 6.1 Token 格式

HMAC-SHA256 签名的紧凑 token，payload 为 base64url 编码的 JSON：

```
<base64url(payload)>.<base64url(hmac_signature)>
```

Payload 结构：
```json
{
  "tenant_id": "<tenant_id>",
  "iat": 1714608000
}
```

### 6.2 中间件逻辑（middleware/auth.rs）

```rust
/// 从 Authorization header 提取 tenant_id，注入 request extensions。
/// 认证失败返回 `GatewayError::Unauthorized`，由 `IntoResponse` 转换为 401 + 结构化错误体。
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, GatewayError> {
    // 1. 跳过 /healthz
    if req.uri().path() == "/healthz" {
        return Ok(next.run(req).await);
    }

    // 2. 提取 Authorization header
    let header = req.headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let token = match header {
        Some(t) => t,
        None => return Err(GatewayError::Unauthorized),
    };

    // 3. 验证签名
    let token = match state.verify_token(token) {
        Some(t) => t,
        None => return Err(GatewayError::Unauthorized),
    };

    // 4. 注入 tenant_id（使用 newtype 避免与其他 String 扩展冲突）
    req.extensions_mut().insert(TenantId(token.tenant_id));

    Ok(next.run(req).await)
}
```

关键约束：
- 认证失败返回 **401 Unauthorized**，响应体 `{"error":{"code":"unauthorized","message":"invalid or missing token"}}`。
  中间件返回 `GatewayError::Unauthorized`，由 `IntoResponse` 统一转换为带 body 的 401，避免直接返回裸 `StatusCode::UNAUTHORIZED`。
- 资源不存在返回 **404 Not Found**，响应体 `{"error":{"code":"not_found","message":"session not found"}}`
- TUI 客户端可通过 `error.code` 精确区分认证失败与资源不存在，向用户展示正确提示
- 每次请求重新 HMAC 验签，无 session cookie
- Token 签发和管理由 tenant crate 的管理功能或运维工具完成，不在此 spec 范围

---

## 7. 限流中间件

### 7.1 算法

Per-tenant token bucket，存储在 `AppState` 的 `DashMap<String, TokenBucket>` 中。

```rust
use std::time::{Duration, Instant};

struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64,       // tokens per second
    last_refill: Instant,
}

/// 限流器：每个租户一个 TokenBucket，存储在 DashMap 中。
/// 不实现 Clone，避免令牌状态分叉。
pub struct RateLimiter {
    buckets: DashMap<String, TokenBucket>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self { buckets: DashMap::new() }
    }

    pub fn check(&self, tenant_id: &str, config: &RateLimitConfig) -> bool {
        // 获取或插入 bucket，执行令牌桶算法
        // ... 实现省略，见代码 ...
    }
}
```

默认参数：5 req/s，burst 10。

> **内存管理**：当前实现无主动清理，适用于单实例 MVP。水平扩展时需替换为 Redis 等外部存储。

### 7.2 中间件逻辑（middleware/rate_limit.rs）

```rust
/// 注入 request extensions 的 tenant_id newtype，避免与其他 String 扩展冲突。
#[derive(Clone, Debug)]
pub struct TenantId(pub String);

pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let tenant_id = req.extensions().get::<TenantId>()
        .map(|t| t.0.clone())
        .unwrap_or_default();  // 无 tenant_id 不限制（/healthz）

    if tenant_id.is_empty() {
        return Ok(next.run(req).await);
    }

    if !state.rate_limiter.check(&tenant_id, &state.config.rate_limit) {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    Ok(next.run(req).await)
}
```

超限返回 `429 Too Many Requests` + `Retry-After: 1` header，响应体 `{"error":{"code":"rate_limited","message":"rate limit exceeded"}}`。

**中间件顺序依赖：** rate_limit 必须在 auth **之后**执行，因为它依赖 `req.extensions().get::<String>()` 获取 `tenant_id`。axum 的 `Router::layer()` 从外到内执行：最后一个 `.layer()` 最先执行。因此需将 auth 放在 api_routes 的最后一个 `.layer()` 使其在 api_routes 内部最先运行：

```rust
// api_routes 内部执行顺序: auth → rate_limit → handler
// auth 最后添加 → 最先执行（注入 tenant_id）→ rate_limit 次执行（读取 tenant_id）
r.layer(rate_limit_middleware).layer(auth_middleware)
```

根 Router 的外层顺序：`CorsLayer` (最后添加) → `TraceLayer` → api_routes。因此全链路顺序为：
`CORS → tracing → auth → rate_limit → handler`。

---

## 8. 错误处理

### 8.1 错误类型（error.rs）

```rust
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error(transparent)]
    Tenant(#[from] TenantError),
    #[error("invalid session id")]
    InvalidSessionId,
    #[error("session not found")]
    SessionNotFound,
    #[error("rate limit exceeded")]
    RateLimited,
    #[error("unauthorized")]
    Unauthorized,
}
```

### 8.2 HTTP 状态码映射

| 错误 | HTTP | 响应体 |
|---|---|---|
| `Tenant(TenantError::TenantNotFound(_))` | 404 | `{"error":{"code":"not_found","message":"..."}}` |
| `Tenant(TenantError::SessionNotFound(_))` | 404 | `{"error":{"code":"not_found","message":"session not found"}}` |
| `Tenant(TenantError::TenantAlreadyExists(_))` | 409 | `{"error":{"code":"conflict","message":"..."}}` |
| `Tenant(TenantError::SessionLimitExceeded{..})` | 429 | `{"error":{"code":"limit_exceeded","message":"..."}}` |
| `Tenant(TenantError::TokenBudgetExceeded{..})` | 429 | `{"error":{"code":"limit_exceeded","message":"..."}}` |
| `Tenant(TenantError::ToolCallRateLimitExceeded{..})` | 429 | `{"error":{"code":"rate_limited","message":"..."}}` |
| `Tenant(TenantError::Internal(_))` | 500 | `{"error":{"code":"internal","message":"internal error"}}` |
| `InvalidSessionId` | 400 | `{"error":{"code":"invalid_request","message":"..."}}` |
| `SessionNotFound` | 404 | `{"error":{"code":"not_found","message":"session not found"}}` |
| `RateLimited` | 429 | `{"error":{"code":"rate_limited","message":"rate limit exceeded"}}` + `Retry-After: 1` header |
| `Unauthorized` | 401 | `{"error":{"code":"unauthorized","message":"invalid or missing token"}}` |

500 响应不暴露内部错误详情，内部 `String` 仅记录到 tracing span。

```rust
impl IntoResponse for GatewayError {
    fn into_response(self) -> axum::response::Response {
        let (status, body) = match &self {
            Self::Tenant(TenantError::TenantNotFound(_)) => (StatusCode::NOT_FOUND, ...),
            Self::Tenant(TenantError::SessionNotFound(_)) => (StatusCode::NOT_FOUND, ...),
            Self::Tenant(TenantError::TenantAlreadyExists(_)) => (StatusCode::CONFLICT, ...),
            Self::Tenant(TenantError::SessionLimitExceeded { .. }) => (StatusCode::TOO_MANY_REQUESTS, ...),
            Self::Tenant(TenantError::TokenBudgetExceeded { .. }) => (StatusCode::TOO_MANY_REQUESTS, ...),
            Self::Tenant(TenantError::ToolCallRateLimitExceeded { .. }) => (StatusCode::TOO_MANY_REQUESTS, ...),
            Self::Tenant(TenantError::Internal(msg)) => {
                tracing::error!(error = %msg, "tenant internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, ...)
            }
            Self::InvalidSessionId => (StatusCode::BAD_REQUEST, ...),
            Self::SessionNotFound => (StatusCode::NOT_FOUND, ...),
            Self::RateLimited => (StatusCode::TOO_MANY_REQUESTS, ...),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, ...),
        };
        // ...
    }
}
```

---

## 9. 可观测性

### 9.1 Tracing

每个请求注入 span：

**推荐方案**：在 `auth_middleware` 验证成功后显式创建 `info_span!`，并作为当前 Span 注入，而非依赖 `TraceLayer::make_span_with`。

原因：`TraceLayer` 附加在根 Router 上，先于 api_routes 内的 auth 中间件执行，此时 `tenant_id` 尚未从 token 中提取。若在 `make_span_with` 中读取 `req.extensions()`，只能得到 `"unknown"`。

```rust
// middleware/auth.rs
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, GatewayError> {
    // ... token 验证逻辑 ...
    let payload = state.verify_token(token).ok_or(GatewayError::Unauthorized)?;

    // 创建带 tenant_id 的 span，使其覆盖后续 handler 和 TenantManager 调用
    let span = tracing::info_span!(
        "http_request",
        http.method = %req.method(),
        http.uri = %req.uri(),
        tenant_id = %payload.tenant_id,
    );
    let _enter = span.enter();

    req.extensions_mut().insert(TenantId(payload.tenant_id));
    Ok(next.run(req).await)
}
```

`session_id` 在 handler 层从 Path 提取后，通过 `tracing::info_span!(parent: &current_span, "handler", session_id = %id)` 创建子 span，确保 SSE 任务等异步操作也携带完整上下文。

所有后续的 `TenantManager` 调用、SSE 任务都在此 span 下，确保 `tenant_id` / `session_id` 贯穿全链路。

> **中间件顺序（修正）**：axum 的 `Router::layer()` 从外到内执行。最后一个 `.layer()` 最先执行。因此正确的顺序是：
> ```rust
> // 全链路执行顺序: CORS → tracing → auth → rate_limit → handler
> // 写法（从外到内）:
> router
>     .layer(CorsLayer::permissive())     // 最后添加 → 最先执行
>     .layer(TraceLayer::new_for_http())  // 次后添加 → 第二执行
>     .nest("/api/v1", api_routes)        // api_routes 内部顺序: auth → rate_limit → handler
> ```
> 
> 注意：auth 和 rate_limit 是附加在 **api_routes** 子 Router 上的：
> ```rust
> let api_routes = Router::new()
>     // ... routes ...
>     .layer(rate_limit_middleware)  // 先添加（在 api_routes 内部后执行）
>     .layer(auth_middleware);        // 后添加（在 api_routes 内部先执行）
> ```
> 因此全链路顺序为 `CORS → tracing → auth → rate_limit → handler`。

### 9.2 指标（后续阶段）

MVP 暂不引入 metrics crate。tracing span 已记录请求耗时和状态码，可通过 `tracing_subscriber` 输出到 stdout 或 OTLP。

---

## 10. 服务启动与优雅关闭

### 10.1 ServerConfig（config.rs）

```rust
use secrecy::SecretString;
use std::net::SocketAddr;

/// 默认测试密钥。生产环境禁止直接使用此值运行。
const DEFAULT_TEST_SECRET: &str = "test-secret-32-chars-long!!!";

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: SocketAddr,          // 默认 0.0.0.0:8080
    pub auth_secret: SecretString,      // HMAC-SHA256 签名密钥
    pub rate_limit: RateLimitConfig,
    pub default_model: String,          // 默认 LLM 模型，填充 SessionInfo.model
    pub default_context_window: u64,    // 默认上下文窗口，填充 SessionInfo.context_window
}

#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub requests_per_second: u32,  // 默认 5
    pub burst_size: u32,           // 默认 10
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], 8080)),
            // 默认密钥仅用于测试。生产环境必须通过环境变量设置。
            auth_secret: SecretString::new(DEFAULT_TEST_SECRET.into()),
            rate_limit: RateLimitConfig::default(),
            default_model: "claude-sonnet-4".to_string(),
            default_context_window: 128_000,
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self { requests_per_second: 5, burst_size: 10 }
    }
}
```

加载优先级：环境变量 `PANDARIA_BIND_ADDR`、`PANDARIA_AUTH_SECRET` > 默认值。不引入配置文件（MVP 运维用环境变量足够）。

### 10.2 AppState

```rust
pub struct AppState {
    pub tenant_manager: Arc<dyn TenantManager>,
    pub config: ServerConfig,
    pub rate_limiter: RateLimiter,
}

impl AppState {
    fn verify_token(&self, token_str: &str) -> Option<TokenPayload> {
        // HMAC-SHA256 验证 + base64url 解码
    }
}
```

### 10.3 Router 组装（server.rs）

```rust
use axum::{Router, middleware};
use tower_http::cors::CorsLayer;

pub fn build_router(state: Arc<AppState>) -> Router {
    let api_routes = Router::new()
        .route("/sessions", post(sessions::create).get(sessions::list))
        .route("/sessions/{id}", get(sessions::get).patch(sessions::update).delete(sessions::delete))
        .route("/sessions/{id}/messages", post(messages::send))
        .route("/sessions/{id}/messages/current", delete(messages::interrupt))
        .route("/sessions/{id}/events", get(events::stream))
        // auth 最后添加 → 最先执行（注入 tenant_id）→ rate_limit 次执行（读取 tenant_id）
        .layer(middleware::from_fn_with_state(state.clone(), rate_limit_middleware))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    Router::new()
        .route("/healthz", get(health::get))
        .nest("/api/v1", api_routes)
        // 执行顺序: CORS → tracing → rate_limit → auth → handler
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())  // MVP，后续收紧
        .with_state(state)
}
```

### 10.4 优雅关闭

```rust
pub async fn serve(state: Arc<AppState>) -> Result<(), Box<dyn std::error::Error>> {
    // 安全启动检查：禁止以默认测试密钥运行在生产环境
    if state.config.auth_secret.expose_secret() == DEFAULT_TEST_SECRET {
        panic!(
            "Default auth secret detected. Set PANDARIA_AUTH_SECRET environment variable."
        );
    }

    let listener = tokio::net::TcpListener::bind(&state.config.bind_addr).await?;
    let router = build_router(state);

    tracing::info!("api-gateway listening on {}", listener.local_addr()?);

    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("shutdown signal received");
        })
        .await?;

    Ok(())
}
```

---

## 11. 测试策略

| 层 | 内容 | 方式 |
|---|---|---|
| `auth_tests.rs` | Token 验证/拒绝、格式异常处理 | 单元测试 |
| `routes_tests.rs` | 端点 CRUD、错误映射 | 集成测试，mock `TenantManager` |
| `sse_tests.rs` | SSE 流发送/断开/重连 | 单元测试 + mock `mpsc` |

Mock TenantManager：
```rust
struct MockTenantManager {
    sessions: Mutex<HashMap<String, SessionInfo>>,
    event_senders: Mutex<HashMap<String, Vec<tokio::sync::mpsc::Sender<AgentEvent>>>>,
}

#[async_trait]
impl TenantManager for MockTenantManager {
    async fn subscribe_events(
        &self,
        _tenant_id: &str,
        session_id: &uuid::Uuid,
    ) -> Result<tokio::sync::mpsc::Receiver<AgentEvent>, TenantError> {
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        self.event_senders.lock().unwrap()
            .entry(session_id.to_string())
            .or_default()
            .push(tx);
        Ok(rx)
    }
    // ... 其他方法实现省略
}
```

测试使用 `axum_test` 或 `tower::util::ServiceExt`，通过 `build_router()` 构造完整的请求链路。

---

## 12. 关键设计决策

| 决策 | 理由 |
|---|---|
| axum + SSE（非 WebSocket/gRPC） | 与 TUI spec 对齐，SSE 是单向推送的自然匹配。HTTP 中间件生态成熟。WS/gRPC 后续按需添加。 |
| TenantManager trait 依赖反转 | 遵守 `api-gateway → tenant` 依赖方向。Gateway 可独立测试（mock trait）。 |
| SSE 每个连接独立 mpsc 订阅 | 每个客户端有独立的事件流状态。连接断开自动清理。避免 broadcast 场景下的背压问题。 |
| 认证失败返回 401（非 404） | TUI 需要向用户展示精确的错误提示（"Auth failed. Run /auth"）。`error.code` 字段区分 `unauthorized` 与 `not_found`，不泄露资源存在性。 |
| SessionInfo 以 api-gateway 定义为权威来源 | TUI 是 gateway 的客户端，协议格式由 gateway 定义。tenant crate 实现 TenantManager 时返回 gateway 的 SessionInfo 类型。 |
| ServerEvent 与 TUI 已实现的反序列化格式对齐 | `TextDelta` 等字段与 TUI `client/model.rs` 完全一致，避免联调时出现 serde 错误。 |
| Token: HMAC 紧凑格式 | 无需 JWT 库依赖。服务端签发服务端验证，不需要自包含 claims。 |
| 限流: per-tenant token bucket | 轻量内存实现，无需外部存储。后续可升级 Redis 滑动窗口。 |
| MVP 无 Swagger/OpenAPI | 端点少，TUI 是唯一已知客户端。后续 API 稳定后补文档。 |

---

## 13. 出界（MVP）

- Swagger / OpenAPI 文档
- gRPC 端点（为服务间通信保留，MVP 不实现）
- WebSocket 升级
- 管理端点（token 签发、租户 CRUD）—— 由 tenant crate 或运维脚本负责
- 请求日志持久化 —— tracing 输出即可
- GraphQL 接口
- TLS 终止 —— 反向代理（nginx/Caddy）处理
- 流式工具输出（`ToolExecutionUpdate` 不映射为 SSE 事件）

---

## 附录 A：TUI 客户端类型对齐检查表

本 spec 的 `ServerEvent` 和 `SessionInfo` 必须与 TUI `crates/tui/src/client/model.rs` 保持字段级兼容。

| 类型 | TUI 字段 | Gateway 字段 | 状态 | 说明 |
|---|---|---|---|---|
| `ServerEvent::TextDelta` | `delta: String` | `delta: String` | ✅ | |
| `ServerEvent::ThinkingDelta` | `content_index: usize, delta: String` | `content_index: usize, delta: String` | ⚠️ | 协议定义已对齐，但当前 agent-core 未生成对应事件，MVP 不会触发 |
| `ServerEvent::ToolCallStarted` | `call_id: String, name: String` | `call_id: String, name: String` | ⚠️ | 同上 |
| `ServerEvent::ToolCallDelta` | `call_id: String, delta: String` | `call_id: String, delta: String` | ⚠️ | 同上 |
| `ServerEvent::ToolCallDone` | `call_id: String, result: Option<String>, is_error: bool` | `call_id: String, result: Option<String>, is_error: bool` | ✅ | `result` 从 `Vec<Content>` 提取 Text 拼接 |
| `ServerEvent::TurnEnd` | `stop_reason: String, usage: Option<UsageInfo>` | `stop_reason: String, usage: Option<UsageInfo>` | ✅ | |
| `ServerEvent::Error` | `code: String, message: String` | `code: String, message: String` | ✅ | |
| `SessionInfo.id` | `String` | `Uuid` | ⚠️ | tenant 返回 `Uuid`，gateway 需 `to_string()` 后转发 |
| `SessionInfo.title` | `Option<String>` | `Option<String>` | ✅ | tenant 已返回，gateway 直接转发 |
| `SessionInfo.model` | `String` | `String` | ✅ | tenant crate 已返回，gateway 直接转发 |
| `SessionInfo.context_window` | `Option<u64>` | `Option<u64>` | ⚠️ | tenant 不返回，gateway 需从 ServerConfig 填充默认值 |
| `SessionInfo.created_at` | `Option<String>` | `String` | ✅ | serde 自动处理 `String` → `Option<String>` 反序列化，无需 gateway 特殊处理 |
| `SessionInfo.turn_count` | — | `u64` | — | TUI 无此字段，gateway 不转发给客户端 |
| `SessionInfo.system_prompt` | — | `Option<String>` | — | TUI 无此字段，gateway 不转发给客户端 |

> **维护者注意**：若未来变更上述字段，必须同步更新 TUI 的 `client/model.rs` 并执行联调测试。

---

## 附录 B：变更记录

| 日期 | 变更 |
|---|---|
| 2026-05-02 | 初版 spec |
| 2026-05-14 | 审查修订：TUI 对齐、401 认证、中间件顺序、RateLimiter、auth_secret 检查、TenantError HTTP 映射、SessionInfo 字段差异标注、AgentEvent 预留事件标注、PATCH update 预留说明 |
| 2026-05-15 | 修正：`CreateSessionParams` / `SessionInfo` 增加 `title`（tenant crate 已同步实现）；auth_secret 启动检查改为针对 `DEFAULT_TEST_SECRET`；`ServerConfig` 增加 `default_model` / `default_context_window`；auth/rate_limit 中间件使用 `TenantId` newtype；`IntoResponse` 伪代码修正为实际 `TenantError` 变体；增加 `agent-core` 直接依赖说明；`SessionInfo.created_at` 兼容性标注修正；增加 `CreateSessionRequest` serde 兼容说明；token payload 中 `tenant_id` 示例改为 `<tenant_id>` |
| 2026-05-15 | 新增 API：`PATCH /sessions/{id}` 从预留改为正式实现；`POST /sessions/{id}/compact`；`GET /sessions/{id}/messages`；`TenantManager` trait 新增 `update_session` / `compact_session` / `get_session_messages`；新增附录 C（TUI 命令映射表） |
| 2026-05-16 | 审查修订：AgentEvent 映射表删除不存在的 `ThinkingStart`/`ThinkingDelta`/`ToolCallStart`/`ToolCallDelta` 变体，补充 `CompactionStart/End` 和 `AutoRetryStart/End` 的 MVP 跳过策略；修正 `SessionInfo.model` 来源描述（tenant 已返回）；明确 tracing span 推荐方案（auth 中间件内创建 `info_span!`） |

---

## 附录 C：TUI 命令 ↔ API 端点映射表

| TUI 命令 | 触发方式 | 对应 API | 状态 | 说明 |
|---|---|---|---|---|
| `/new [title]` | 输入 `/new` 或 `Ctrl+N` | `POST /api/v1/sessions` | ✅ | 创建 session |
| `/switch <id>` | 输入 `/switch abc` | `GET /api/v1/sessions/{id}` | ✅ | 切换并刷新 session 元数据 |
| `/list` / `Ctrl+S` | 输入 `/list` | —（本地） | ⚠️ | 仅展示本地缓存，**未调 `GET /api/v1/sessions`** |
| `/connect <url>` | 输入 `/connect` | `GET /api/v1/sessions` | ✅ | 连接测试 |
| `/auth <token>` | 输入 `/auth` | `GET /api/v1/sessions` | ✅ | 设置 token 后做连接测试 |
| `/rename <title>` | 输入 `/rename` | `PATCH /api/v1/sessions/{id}` | ✅ | 重命名 session |
| `/model [id]` | 输入 `/model` | `PATCH /api/v1/sessions/{id}`（有 id 时） | ⚠️ | 当前仅本地修改，未调 PATCH |
| `/compact` | 输入 `/compact` | `POST /api/v1/sessions/{id}/compact` | ✅ | 触发上下文压缩 |
| `/retry` | 输入 `/retry` | `POST /messages` + SSE | ✅ | 重发上一条消息 |
| `Enter`（输入文本） | 按 Enter | `POST /messages` + SSE | ✅ | 发送消息并接收 SSE 流 |
| `Escape`（streaming） | 按 Esc | `DELETE /messages/current` | ✅ | 中断当前 turn |
| `/tokens` | 输入 `/tokens` | —（本地） | — | 本地 token 统计，无 API 调用 |
| `/copy` | 输入 `/copy` | —（本地） | — | 剪贴板操作，无 API 调用 |
| `/dump [file]` | 输入 `/dump` | —（本地） | — | 本地文件导出，无 API 调用 |
| `/clear` | 输入 `/clear` | —（本地） | — | 清空本地消息列表 |
| `/quit` | 输入 `/quit` | — | — | 退出应用 |
| Session List `Delete` | 按 `Delete`/`d` | — | ⚠️ | TUI 返回 `delete:session_id` 但未处理，**未调 `DELETE /sessions/{id}`** |

### 已知缺口

1. **TUI `/list` 未刷新服务器数据**：`GET /api/v1/sessions` 存在，但 TUI 的 `/list` 命令只打开本地 overlay。
2. **TUI `/model` 未持久化到服务器**：`PATCH` 已支持 `model` 字段，但 TUI 仅在本地修改 `session.info.model`。
3. **TUI Session List 删除未完成**：overlay 返回 `delete:session_id`，但 `handle_overlay_confirm` 无法解析该格式，未调用 `DELETE /sessions/{id}`。
4. **TUI 缺少历史消息恢复**：`GET /api/v1/sessions/{id}/messages` 已定义，但 TUI 未在切换 session 或重连时调用以恢复历史消息。
