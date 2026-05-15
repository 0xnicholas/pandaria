# api-gateway 实现计划

> **Date:** 2026-05-14
> **Status:** Draft
> **Spec Reference:** `docs/specs/2026-05-02-api-gateway.md`（修订版）
> **Priority:** P1 — 依赖 `crates/tenant/` 完成后可全面启动

---

## 目标

实现 `crates/api-gateway/`，作为 pandaria 的服务端 HTTP 入口层，提供 REST API + SSE 事件流，对接 TUI 客户端。

---

## 前置依赖

| 依赖 | 状态 | 说明 |
|---|---|---|
| `crates/tenant/` | 🟡 计划中 | `TenantManager` trait 及 `TenantError` 定义 |
| `crates/agent-core/` | ✅ 已完成 | `AgentEvent` 类型定义 |
| `crates/tui/` | ✅ 已实现 | 客户端协议基准（字段必须与 TUI 对齐） |

---

## 文件结构

```
crates/api-gateway/
  Cargo.toml
  README.md
  src/
    lib.rs                     # re-exports, server bootstrap fn
    server.rs                  # axum Router assembly, graceful shutdown
    config.rs                  # ServerConfig, RateLimitConfig
    types.rs                   # ServerEvent, SessionInfo, UsageInfo, ApiError
    error.rs                   # GatewayError, IntoResponse
    sse.rs                     # SseStream: axum IntoResponse
    routes/
      mod.rs
      sessions.rs              # POST create, GET list, GET, PATCH update, DELETE
      messages.rs              # POST send, DELETE interrupt
      events.rs                # GET SSE stream
      health.rs                # GET /healthz
    middleware/
      mod.rs
      auth.rs                  # Bearer token → tenant_id (HMAC-SHA256)
      rate_limit.rs            # per-tenant token bucket
      tracing_mw.rs            # tenant_id/session_id span injection
  tests/
    auth_tests.rs              # Token validation
    routes_tests.rs            # Endpoint integration with mock TenantManager
    sse_tests.rs               # SSE stream send/close/disconnect
```

---

## 任务分解

### Task 1: 脚手架（~30min）

- [ ] 创建 `crates/api-gateway/Cargo.toml`
- [ ] 创建 `crates/api-gateway/src/lib.rs`
- [ ] 更新 workspace `Cargo.toml`
- [ ] 运行 `cargo check -p api-gateway`

### Task 2: 基础类型（~1h）

- [ ] `src/types.rs` — ServerEvent, SessionInfo, UsageInfo, ApiError
- [ ] `src/error.rs` — GatewayError + IntoResponse
- [ ] `src/config.rs` — ServerConfig, RateLimitConfig
- [ ] 编写单元测试验证 serde 往返

### Task 3: 认证中间件（~1.5h）

- [ ] `src/middleware/auth.rs`
- [ ] HMAC-SHA256 验证逻辑
- [ ] 401 响应格式
- [ ] 单元测试

### Task 4: 限流中间件（~1h）

- [ ] `src/middleware/rate_limit.rs`
- [ ] `TenantId` newtype，避免 extension key 冲突
- [ ] RateLimiter + TokenBucket
- [ ] 429 响应 + Retry-After
- [ ] 单元测试

### Task 5: Tracing 中间件（~30min）

- [ ] `src/middleware/tracing_mw.rs`
- [ ] 在 auth 成功后创建带 `tenant_id` / `session_id` 的 tracing span
- [ ] 或使用 `tower_http::trace::TraceLayer` + `make_span_with` 闭包

### Task 6: SSE 流（~1h）

- [ ] `src/sse.rs` — SseStream
- [ ] 15s keep-alive
- [ ] 单元测试

### Task 7: 路由层（~2h）

- [ ] health, sessions, messages, events
- [ ] PATCH `sessions::update` — 部分更新 title / model / system_prompt
- [ ] POST `sessions::compact` — 触发上下文压缩
- [ ] GET `sessions::messages` — 获取历史消息列表
- [ ] 集成测试（tower::ServiceExt + mock TenantManager）

### Task 8: 服务启动与组装（~1h）

- [ ] `src/server.rs`
- [ ] 中间件顺序修正
- [ ] auth_secret 启动 panic
- [ ] 优雅关闭

### Task 9: 联调验证（~1h）

- [ ] 启动 gateway + mock
- [ ] TUI 连接验证
- [ ] 字段级对齐检查

---

## 技术要点

### 依赖约束
- **禁止**直接依赖 ai-provider / llm_client
- UsageInfo 在 gateway 内部定义
- ServerEvent 所有字段必须与 TUI client/model.rs 对齐

### 认证
- Token: `<base64url(payload)>.<base64url(hmac_signature)>`
- Payload: `{ "tenant_id": "...", "iat": ... }`
- 失败返回 **401** + error.code = "unauthorized"
- auth 中间件返回 `GatewayError::Unauthorized`，由 `IntoResponse` 统一转换为带结构化 error body 的 401，不直接返回裸 `StatusCode::UNAUTHORIZED`
- auth 中间件注入 `TenantId` newtype（而非裸 `String`）到 request extensions，避免与其他中间件冲突

### 中间件顺序（重要）
```rust
// 全链路执行顺序: CORS → tracing → auth → rate_limit → handler
let api_routes = Router::new()
    // ... routes ...
    .layer(middleware::from_fn_with_state(state.clone(), rate_limit_middleware))
    .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

Router::new()
    .route("/healthz", get(health::get))
    .nest("/api/v1", api_routes)
    .layer(TraceLayer::new_for_http())
    .layer(CorsLayer::permissive())
    .with_state(state)
```

---

## 测试策略

| 测试文件 | 覆盖内容 |
|---|---|
| `tests/auth_tests.rs` | HMAC 验证、格式异常 |
| `tests/routes_tests.rs` | 端点 CRUD、错误码映射 |
| `tests/sse_tests.rs` | SSE 流、断开、序列化 |

---

## 验收标准

```bash
cargo check -p api-gateway
cargo test -p api-gateway
```

## 相关文档

- `docs/specs/2026-05-02-api-gateway.md` — 详细规格（修订版）
- `docs/specs/2026-05-02-tui-design.md` — TUI 客户端规格
- `AGENTS.md` — 架构决策

## 变更记录

| 日期 | 变更 |
|---|---|
| 2026-05-02 | 初版 spec |
| 2026-05-14 | 审查修订：TUI 对齐、401 认证、中间件顺序、RateLimiter、auth_secret 检查 |
| 2026-05-14 | 文档修正：TenantError 变体与 HTTP 映射、SessionInfo 字段差异标注、AgentEvent 预留事件标注、PATCH update 预留说明 |
| 2026-05-15 | 修正：tenant crate `CreateSessionParams` / `SessionInfo` 增加 `title`；auth_secret 检查针对默认测试密钥；`ServerConfig` 增加 `default_model` / `default_context_window`；`TenantId` newtype；IntoResponse 伪代码修正为实际 `TenantError` 变体；增加 `agent-core` 依赖说明；增加 `tracing_mw.rs` 任务；增加 `CreateSessionRequest` serde 兼容说明 |
| 2026-05-15 | 新增 API 设计：`PATCH /sessions/{id}` 从预留改为正式实现；`POST /sessions/{id}/compact`；`GET /sessions/{id}/messages`；`TenantManager` trait 新增 `update_session` / `compact_session` / `get_session_messages` |
