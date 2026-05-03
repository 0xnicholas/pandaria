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
api-gateway → tenant → extensions → agent-core → llm-client
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

[dev-dependencies]
tokio-test = "0.4"
tower = { version = "0.5", features = ["util"] }
```

---

## 3. TenantManager trait（依赖反转边界）

Gateway 通过此 trait 与 tenant/session 层交互。定义在 tenant crate 中，此处列为 Gateway 视角的依赖接口。

```rust
use async_trait::async_trait;
use agent_core::events::AgentEvent;
use uuid::Uuid;

/// 由 tenant crate 实现，Gateway 通过 Arc<dyn TenantManager> 注入。
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionParams {
    pub system_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: Uuid,
    pub tenant_id: String,
    pub created_at: String,
    pub turn_count: u64,
    pub system_prompt: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum TenantError {
    #[error("session not found")]
    NotFound,
    #[error("tenant limit exceeded")]
    LimitExceeded,
    #[error("internal error: {0}")]
    Internal(String),
}
```

关键设计点：
- `send_message` 返回 `u64`（turn_index），不等待 turn 完成。SSE 连接会收到后续事件。
- `subscribe_events` 每个 SSE 连接独立调用，返回私有的 `mpsc::Receiver`，断开即取消。
- `AgentEvent` 来自 `agent-core`，Gateway 只做映射转发，不解析事件语义。
- `TenantError` 映射为 HTTP 404 / 429 / 500。

**实现依赖排序：** `TenantManager` trait 及其关联类型（`TenantError`、`CreateSessionParams`、`SessionInfo`）正式定义在 `crates/tenant/` 中（尚未实现）。Gateway 的 `routes` 层可在 tenant crate 可用前先通过 mock `TenantManager` 开发测试。生产就绪依赖 `crates/tenant/` 完成。

---

## 4. API 端点

### 4.1 端点表

| 方法 | 路径 | Handler | 认证 |
|---|---|---|---|
| `GET` | `/healthz` | `health::get` | 无 |
| `POST` | `/api/v1/sessions` | `sessions::create` | Bearer |
| `GET` | `/api/v1/sessions` | `sessions::list` | Bearer |
| `GET` | `/api/v1/sessions/{id}` | `sessions::get` | Bearer |
| `POST` | `/api/v1/sessions/{id}/messages` | `messages::send` | Bearer |
| `DELETE` | `/api/v1/sessions/{id}/messages/current` | `messages::interrupt` | Bearer |
| `GET` | `/api/v1/sessions/{id}/events` | `events::stream` | Bearer |

### 4.2 Request/Response

**POST /api/v1/sessions**
```
Request:  { "system_prompt": "..." | null }
Response: 201 { "id": "uuid", "tenant_id": "...", "created_at": "...",
                "turn_count": 0, "system_prompt": "..." }
```

**GET /api/v1/sessions**
```
Response: 200 [ SessionInfo, ... ]
```

**GET /api/v1/sessions/{id}**
```
Response: 200 SessionInfo | 404
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

安全约束：认证失败、资源不存在统一返回 404，不泄露资源存在性。只有 healthz 返回 200 无认证。

---

## 5. SSE 事件协议

### 5.1 ServerEvent 类型

```rust
use llm_client::Usage;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ServerEvent {
    /// 流式文本增量
    TextDelta {
        message_index: u64,
        delta: String,
    },
    /// LLM 开始调用工具
    ToolCallStarted {
        call_id: String,
        name: String,
        arguments: serde_json::Value,
    },
    /// 工具参数流式增量
    ToolCallDelta {
        call_id: String,
        delta: String,
    },
    /// 工具执行完成（含结果）
    ToolCallDone {
        call_id: String,
        result: serde_json::Value,
        is_error: bool,
    },
    /// 当前 turn 结束
    TurnEnd {
        turn_index: u64,
        stop_reason: String,
        usage: Usage,
    },
    /// Agent loop 整体结束
    AgentEnd {
        messages_count: u64,
    },
    /// 错误事件
    Error {
        code: String,
        message: String,
    },
}
```

### 5.2 AgentEvent → ServerEvent 映射

agent-core 定义的 `AgentEvent` 变体（参考 `docs/specs/2026-05-02-agent-core.md` 1.1 节）：

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
    Error { error: AgentError },
}
```

| AgentEvent | ServerEvent | 映射逻辑 |
|---|---|---|
| `MessageUpdate { message_index, content_delta }` | `TextDelta { message_index, delta }` | `delta = content_delta`，直通 |
| `ToolExecutionStart { tool_call_id, tool_name }` | `ToolCallStarted { call_id, name, arguments }` | `call_id = tool_call_id`, `name = tool_name`。**`arguments` 从 `MessageEnd` 的 `AgentMessage` 中提取**：Gateway 需缓存最近 `MessageEnd.message`，按 `tool_call_id` 查找对应 `Content::ToolCall(ToolCall { arguments })`。 |
| `ToolExecutionUpdate { tool_call_id, content }` | `ToolCallDelta { call_id, delta }` | `call_id = tool_call_id`, `delta = content`，直通 |
| `ToolExecutionEnd { tool_call_id, result }` | `ToolCallDone { call_id, result, is_error }` | `call_id = tool_call_id`, `result = result.content` 序列化为 `Value`，`is_error = result.is_error` |
| `TurnEnd { turn_index, messages }` | `TurnEnd { turn_index, stop_reason, usage }` | `stop_reason` 和 `usage` 从 `messages` 中**最后一个 `AssistantMessage`** 提取：`stop_reason = assistant.stop_reason.to_string()`, `usage = assistant.usage` |
| `AgentEnd { messages }` | `AgentEnd { messages_count }` | `messages_count = messages.len() as u64` |
| `Error { error }` | `Error { code, message }` | `code = error.variant_name()`, `message = error.to_string()` |
| `AgentStart` / `TurnStart` / `MessageStart` / `MessageEnd` | — | 不发送（客户端只需 delta，`MessageEnd` 仅被 Gateway 内部用于缓存 assistant message 以提取 tool arguments） |

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
  "tenant_id": "<uuid>",
  "iat": 1714608000
}
```

### 6.2 中间件逻辑（middleware/auth.rs）

```rust
/// 从 Authorization header 提取 tenant_id，注入 request extensions
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
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
        None => return Err(StatusCode::NOT_FOUND),
    };

    // 3. 验证签名
    let token = match state.verify_token(token) {
        Some(t) => t,
        None => return Err(StatusCode::NOT_FOUND),
    };

    // 4. 注入 tenant_id
    req.extensions_mut().insert(token.tenant_id);

    Ok(next.run(req).await)
}
```

关键约束：
- 认证失败返回 **404**（非 401），与资源不存在不可区分，防止旁路探测
- 每次请求重新 HMAC 验签，无 session cookie
- Token 签发和管理由 tenant crate 的管理功能或运维工具完成，不在此 spec 范围

> **与 TUI spec 对齐说明：** TUI spec (Section 12) 假设认证失败返回 401 以显示 "Auth failed. Run /auth" 提示。Gateway 选择 404 优先安全性。TUI 的 `rest.rs` 应通过 404 响应的 `error.code` 字段区分（`"not_found"` vs 未来的 `"unauthorized"`），而非依赖 HTTP 状态码。若后续需求变化，可为认证失败添加独立响应码（如 401 + 特定 `error.code`）。

---

## 7. 限流中间件

### 7.1 算法

Per-tenant token bucket，存储在 `AppState` 的 `DashMap<String, TokenBucket>` 中。

```rust
use std::time::{Duration, Instant};

#[derive(Clone)]
struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64,       // tokens per second
    last_refill: Instant,
}
```

默认参数：5 req/s，burst 10。

### 7.2 中间件逻辑（middleware/rate_limit.rs）

```rust
pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let tenant_id = req.extensions().get::<String>()
        .cloned()
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

**中间件顺序依赖：** rate_limit 必须在 auth 之后执行，因为它依赖 `req.extensions().get::<String>()` 获取 `tenant_id`。axum 的 `Router::layer()` 从外到内执行：最后一个 `.layer()` 最先执行。因此需将 auth 放在最后一个 `.layer()` 使其最先运行：

```
// 正确：auth 最后添加 → 最先执行（注入 tenant_id）→ rate_limit 次执行（读取 tenant_id）
r.layer(rate_limit_middleware).layer(auth_middleware)
```

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
}
```

### 8.2 HTTP 状态码映射

| 错误 | HTTP | 响应体 |
|---|---|---|
| `Tenant(TenantError::NotFound)` | 404 | `{"error":{"code":"not_found","message":"..."}}` |
| `Tenant(TenantError::LimitExceeded)` | 429 | `{"error":{"code":"limit_exceeded","message":"..."}}` |
| `Tenant(TenantError::Internal(_))` | 500 | `{"error":{"code":"internal","message":"internal error"}}` |
| `InvalidSessionId` | 400 | `{"error":{"code":"invalid_request","message":"..."}}` |
| `SessionNotFound` | 404 | `{"error":{"code":"not_found","message":"session not found"}}` |
| `RateLimited` | 429 | `{"error":{"code":"rate_limited","message":"rate limit exceeded"}}` + `Retry-After: 1` header |

500 响应不暴露内部错误详情，内部 `String` 仅记录到 tracing span。

```rust
impl IntoResponse for GatewayError {
    fn into_response(self) -> axum::response::Response {
        let (status, body) = match &self {
            Self::Tenant(TenantError::NotFound) => (StatusCode::NOT_FOUND, ...),
            Self::Tenant(TenantError::LimitExceeded) => (StatusCode::TOO_MANY_REQUESTS, ...),
            Self::Tenant(TenantError::Internal(msg)) => {
                tracing::error!(error = %msg, "tenant internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, ...)
            }
            Self::InvalidSessionId => (StatusCode::BAD_REQUEST, ...),
            Self::SessionNotFound => (StatusCode::NOT_FOUND, ...),
            Self::RateLimited => (StatusCode::TOO_MANY_REQUESTS, ...),
        };
        // ...
    }
}
```

---

## 9. 可观测性

### 9.1 Tracing

每个请求注入 span：
```rust
// middleware/tracing_mw.rs
// 使用 tower_http::trace::TraceLayer 记录请求属性。
// session_id 从 URI path 中提取（如 /api/v1/sessions/{id} → 提取 {id}）：
//   path_segments = req.uri().path().split('/')
//   session_id = path_segments.nth(3)  // segments: ["", "api", "v1", "sessions", "{id}", ...]
//
//   span = info_span!("http_request",
//     http.method = %req.method(),
//     http.uri = %req.uri(),
//     tenant_id = %req.extensions().get::<String>().unwrap_or("unknown"),
//     session_id = %session_id.unwrap_or("-"),
//   )
```

所有后续的 `TenantManager` 调用、SSE 任务都在此 span 下，确保 `tenant_id` 贯穿全链路。

### 9.2 指标（后续阶段）

MVP 暂不引入 metrics crate。tracing span 已记录请求耗时和状态码，可通过 `tracing_subscriber` 输出到 stdout 或 OTLP。

---

## 10. 服务启动与优雅关闭

### 10.1 ServerConfig（config.rs）

```rust
use secrecy::SecretString;
use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: SocketAddr,          // 默认 0.0.0.0:8080
    pub auth_secret: SecretString,      // HMAC-SHA256 签名密钥
    pub rate_limit: RateLimitConfig,
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
            auth_secret: SecretString::new("change-me-in-production".into()),
            rate_limit: RateLimitConfig::default(),
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
        .route("/sessions/{id}", get(sessions::get))
        .route("/sessions/{id}/messages", post(messages::send))
        .route("/sessions/{id}/messages/current", delete(messages::interrupt))
        .route("/sessions/{id}/events", get(events::stream))
        .layer(middleware::from_fn_with_state(state.clone(), rate_limit_middleware))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    Router::new()
        .route("/healthz", get(health::get))
        .nest("/api/v1", api_routes)
        .layer(middleware::from_fn(tracing_mw::inject_span))
        .layer(CorsLayer::permissive())  // MVP，后续收紧
        .with_state(state)
}
```

### 10.4 优雅关闭

```rust
pub async fn serve(state: Arc<AppState>) -> Result<(), Box<dyn std::error::Error>> {
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
    sessions: Mutex<HashMap<Uuid, SessionInfo>>,
    events: Mutex<HashMap<Uuid, tokio::sync::broadcast::Sender<AgentEvent>>>,
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
| 认证失败返回 404 | 统一 401/404 响应，防止租户枚举攻击。 |
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
