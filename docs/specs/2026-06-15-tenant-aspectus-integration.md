# Spec: Tenant → Aspectus 统一身份集成

> **日期**: 2026-06-15  
> **状态**: 设计中  
> **关联**: ADR-003（Aspectus 配额管理）、ADR-004（Emerald entity_id 映射）

---

## 1. 目标

将 `tenant` crate 从「自行管理租户注册与配额配置」改造为「消费 Aspectus 作为唯一租户信息来源」。`api-gateway` 的认证中间件从 HMAC-SHA256 自签名 token 替换为 Aspectus Token Introspection (RFC 7662)。

### 非目标

- 不改造 `agent-core`、`ai-provider`、`storage` crate
- 不实现 Aspectus 管理 API 的调用（仅消费 `/introspect` 端点）
- 不实现 OAuth2 流程
- 不改变 Session 生命周期逻辑
- 不改变 Emerald entity_id 映射（Phase 2 单独处理）

---

## 2. 架构变更

### 2.1 Crate 依赖变更

```
改造前：
  api-gateway → tenant → agent-core → ai-provider
                    ↓
                storage

改造后：
  api-gateway → aspectus-client ──HTTP──► Aspectus Server
       │
       ├── tenant → agent-core → ai-provider
       │      ↓
       │  storage
       │
       └── aspectus-client 复用（同一 reqwest::Client 实例）
```

`api-gateway` 新增直接依赖 `aspectus-client`。`tenant` crate 不直接依赖 `aspectus-client`，而是通过 trait 抽象接收 `TenantContext`（由 `api-gateway` 在 auth middleware 中获取并注入）。

### 2.2 数据流

```
Client Request
  │  Authorization: Bearer pk_live_xxx
  ▼
api-gateway auth middleware
  │  1. AspectusClient::introspect("pk_live_xxx")
  │     → IntrospectResponse { tenant_id, user_id, scopes, quotas }
  │  2. 构建 TenantContext，注入 request extensions
  ▼
route handler (sessions / messages)
  │  3. 从 extensions 提取 TenantContext
  │  4. TenantManager::create_session(tenant_context, params)
  │     → TenantRegistry::resolve_or_insert(tenant_context)
  │     → TenantSupervisor::acquire_slot()  // 使用 quotas.pandaria.max_concurrent_sessions
  │     → 创建 SessionActor
  ▼
SessionActor (agent-core)
  │  正常运行，不感知 Aspectus
```

### 2.3 配额流

```
introspect 响应: { quotas: { "pandaria": { "max_concurrent_sessions": 10 } } }
                         │
                         ▼
TenantContext::from_introspect(response)
                         │
                         ▼
TenantSupervisor::new(context)
  → quota.max_concurrent_sessions = context.pandaria_quota().max_concurrent_sessions

运行时：
  - SessionGuard::acquire() → supervisor.try_acquire_slot()
  - CostTracker::record_tokens() → 本地计量（配额上限从 context 读取）
```

---

## 3. 类型设计

### 3.1 TenantContext（新增：`tenant/src/context.rs`）

```rust
use std::time::Instant;

/// 从 Aspectus introspect 响应提取的租户上下文。
/// 由 api-gateway auth middleware 构建，注入 request extensions。
#[derive(Debug, Clone)]
pub struct TenantContext {
    pub tenant_id: String,
    pub user_id: Option<String>,
    pub scopes: Vec<String>,
    pub quotas: TenantQuota,
    /// 缓存创建时间，用于 TTL 判断
    pub cached_at: Instant,
}

impl TenantContext {
    /// 从 Aspectus IntrospectResponse 构建。
    /// 解析 quotas.pandaria JSON 为 TenantQuota。
    pub fn from_introspect(response: &IntrospectResponse) -> Result<Self, TenantError>;

    /// 检查 scopes 是否包含指定权限。
    pub fn has_scope(&self, scope: &str) -> bool;

    /// 缓存是否过期（默认 TTL: 60s）。
    pub fn is_stale(&self, ttl: Duration) -> bool;
}
```

### 3.2 TenantRegistry（重写）

```rust
/// 租户注册表 — Aspectus 适配层。
///
/// 不再自行创建租户。所有租户通过 `resolve_or_insert(TenantContext)` 懒加载。
/// 内部使用 DashMap 缓存已解析的 TenantSupervisor。
///
/// 缓存 TTL 为 300s（5 分钟）——比 auth middleware 的 TenantCache TTL（60s）更长，
/// 因为 TenantSupervisor 包含活跃 session 计数器等运行时状态，频繁重建会丢失计量数据。
pub struct TenantRegistry {
    tenants: DashMap<String, Arc<TenantSupervisor>>,
    cache_ttl: Duration,  // 默认 300s
}

impl TenantRegistry {
    /// 新建空注册表。
    pub fn new(cache_ttl: Duration) -> Self;

    /// 获取或创建 TenantSupervisor。
    /// - 缓存命中且未过期 → 返回已有 supervisor
    /// - 缓存未命中或过期 → 从 TenantContext 创建新 supervisor，替换缓存
    pub fn resolve_or_insert(
        &self,
        ctx: &TenantContext,
    ) -> Result<Arc<TenantSupervisor>, TenantError>;

    /// 强制刷新指定租户的缓存（从 Aspectus 重新获取后调用）。
    pub fn refresh(&self, ctx: &TenantContext) -> Result<Arc<TenantSupervisor>, TenantError>;

    /// 查找已缓存的租户 supervisor。
    pub fn get(&self, tenant_id: &str) -> Option<Arc<TenantSupervisor>>;

    /// 移除指定租户的缓存。
    pub fn evict(&self, tenant_id: &str) -> Option<Arc<TenantSupervisor>>;

    /// 注册租户数。
    pub fn len(&self) -> usize;

    /// 是否为空。
    pub fn is_empty(&self) -> bool;
}
```

### 3.3 TenantQuota（修改）

```rust
/// 资源配额（从 Aspectus quotas JSON 解析）。
#[derive(Debug, Clone, PartialEq)]
pub struct TenantQuota {
    pub max_concurrent_sessions: u32,
    pub max_tokens_per_day: u64,
    pub max_tool_calls_per_minute: u32,
    pub cpu_time_budget_ms_per_day: u64,
}

impl TenantQuota {
    /// 从 Aspectus quotas.pandaria JSON 解析。
    /// 缺失字段使用以下默认值：
    ///   max_concurrent_sessions: 10
    ///   max_tokens_per_day: 1_000_000
    ///   max_tool_calls_per_minute: 60
    ///   cpu_time_budget_ms_per_day: 3_600_000
    pub fn from_aspectus_quotas(quotas: &serde_json::Value) -> Self;
}
```

### 3.4 TenantSupervisor（修改）

```rust
pub struct TenantSupervisor {
    tenant_id: String,
    quota: TenantQuota,         // 来源改为 TenantContext（不再从 Tenant 结构体）
    active_sessions: AtomicU32,
    // ... 其余字段不变
}

impl TenantSupervisor {
    /// 从 TenantContext 创建（替代旧的 Tenant::new）。
    pub fn from_context(ctx: &TenantContext) -> Self;
}
```

### 3.5 TenantError（新增变体）

```rust
// 现有变体保留不变：
// - TenantAlreadyExists(String)
// - TenantNotFound(String)
// - SessionLimitExceeded { tenant_id: String, max: u32, current: u32 }
// - TokenBudgetExceeded { tenant_id: String, consumed: u64, budget: u64 }
// - ToolCallRateLimitExceeded { tenant_id: String, calls: usize }
// - SessionNotFound(String)
// - Internal { tenant_id: String, message: String }

// 新增变体
pub enum TenantError {
    // ... 现有变体 ...

    /// Aspectus introspection 返回 inactive.
    #[error("token introspection failed: inactive token")]
    IntrospectionInactive,
    /// quotas 字段缺失或格式错误.
    #[error("invalid quotas format in introspection response: {0}")]
    InvalidQuotasFormat(String),
    /// 租户未在 Aspectus 配置 pandaria 配额.
    #[error("tenant {0} not configured for pandaria in Aspectus")]
    TenantNotConfigured(String),
}
```

### 3.6 移除的类型

| 类型 | 位置 | 替代 |
|---|---|---|
| `Tenant` 结构体 | `tenant/src/tenant.rs` | `TenantContext` |
| `QuotaCheck` 枚举 | `tenant/src/tenant.rs` | 内联到 `TenantSupervisor` 方法签名 |
| `TenantQuota::default()` | `tenant/src/tenant.rs` | `TenantQuota::from_aspectus_quotas()` |
| `TokenPayload` | `api-gateway` middleware | `aspectus_core::IntrospectResponse` |
| `verify_token()` | `api-gateway` middleware | `AspectusClient::introspect()` |
| `TenantManagerImpl::new()` 中的 global default 参数 | `tenant/src/manager.rs` | 不需要移除——当前构造函数已无 default_* 参数，所有默认值通过 `HarnessConfig` 注入 |

---

## 4. api-gateway 变更

### 4.1 GatewayError 新增变体

`crates/api-gateway/src/error.rs` 需要新增以下变体：

```rust
pub enum GatewayError {
    // 现有变体保留...
    Tenant(TenantError),
    InvalidSessionId,
    SessionNotFound,
    RateLimited,
    Unauthorized,
    NotAcceptable,

    // 新增
    /// Aspectus introspection 失败或超时 → 503
    ServiceUnavailable,
    /// 租户未配置 pandaria 服务 → 403
    Forbidden(String),
    /// 内部错误（TenantContext 解析失败等） → 500
    Internal(String),
}
```

### 4.2 auth middleware 重写

```rust
// 改造前
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, GatewayError> {
    let token_str = extract_bearer_token(&req)?;
    let payload = verify_token(token_str, &state.config.auth_secret)?;
    let tenant_id = payload.tenant_id;
    req.extensions_mut().insert(TenantId(tenant_id));
    // ...
}

// 改造后
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, GatewayError> {
    let token_str = extract_bearer_token(&req)?;

    // 检查本地缓存（TTL 60s），减少 Aspectus 调用
    if let Some(cached) = state.tenant_cache.get(token_str) {
        if !cached.is_stale(Duration::from_secs(60)) {
            req.extensions_mut().insert(cached.ctx.clone());
            return Ok(next.run(req).await);
        }
    }

    // 调用 Aspectus introspection（内建重试）
    let introspect = introspect_with_retry(&state.aspectus, token_str).await
        .map_err(|e| {
            tracing::warn!(error = %e, "aspectus introspection failed");
            GatewayError::ServiceUnavailable
        })?;

    if !introspect.active {
        return Err(GatewayError::Unauthorized);
    }

    let ctx = TenantContext::from_introspect(&introspect)
        .map_err(|e| {
            tracing::error!(error = %e, tenant_id = ?introspect.tenant_id, "invalid introspect response");
            match e {
                TenantError::TenantNotConfigured(_) => {
                    GatewayError::Forbidden("tenant not configured for pandaria".into())
                }
                _ => GatewayError::Internal(e.to_string()),
            }
        })?;

    // 写入缓存
    state.tenant_cache.insert(token_str.to_string(), CachedContext::new(ctx.clone()));

    // 注入 extensions（兼容 rate_limit_middleware）
    req.extensions_mut().insert(ctx.tenant_id.clone());  // TenantId(String) — 兼容现有 rate limiter
    req.extensions_mut().insert(ctx);                     // TenantContext — 供 route handler 使用

    // tracing span
    let span = tracing::info_span!(
        "http_request",
        http.method = %req.method(),
        http.uri = %req.uri(),
        tenant_id = %ctx.tenant_id,
    );
    Ok(async move { next.run(req).await }.instrument(span).await)
}
```

### 4.3 AppState 新增字段

```rust
pub struct AppState {
    // 现有字段保留...
    pub tenant_manager: Arc<TenantManagerImpl>,
    pub config: AppConfig,

    // 新增
    pub aspectus: AspectusClient,
    /// Token → TenantContext 本地缓存（减少 introspect 调用）
    pub tenant_cache: TenantCache,
}
```

### 4.4 AspectusClient 重试封装

`aspectus-client` 当前无内建重试。在 `api-gateway` 中封装重试逻辑：

```rust
/// 调用 Aspectus introspection，指数退避重试最多 2 次。
async fn introspect_with_retry(
    client: &AspectusClient,
    token: &str,
) -> Result<IntrospectResponse, ClientError> {
    let mut attempts = 0;
    loop {
        match client.introspect(token).await {
            Ok(resp) => return Ok(resp),
            Err(e) if attempts < 2 => {
                attempts += 1;
                let delay = Duration::from_millis(100 * 2u64.pow(attempts - 1));
                tokio::time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}
```

### 4.5 TenantCache

```rust
use dashmap::DashMap;
use std::time::Instant;

struct CachedContext {
    ctx: TenantContext,
    cached_at: Instant,
}

pub struct TenantCache {
    entries: DashMap<String, CachedContext>,
    /// 每 N 次查询触发一次过期清理
    check_counter: AtomicU64,
}

impl TenantCache {
    pub fn get(&self, token: &str) -> Option<TenantContext> {
        // 每 1024 次查询触发一次全量清理（对齐 RateLimiter 模式）
        if self.check_counter.fetch_add(1, Ordering::Relaxed) % 1024 == 0 {
            let now = Instant::now();
            self.entries.retain(|_, v| now.duration_since(v.cached_at) < Duration::from_secs(300));
        }
        self.entries.get(token)
            .filter(|v| !v.is_stale(Duration::from_secs(60)))
            .map(|v| v.ctx.clone())
    }

    pub fn insert(&self, token: String, ctx: TenantContext);
    pub fn len(&self) -> usize;
}
```

### 4.5 移除

- `auth.rs` 中的 `verify_token()`、`base64_decode_urlsafe()`、`TokenPayload`
- `AppConfig.auth_secret` 字段（或标记 deprecated）
- HMAC 相关依赖（若不再被其他模块使用）：`hmac`、`sha2` crate

### 4.6 服务启动时 AppState 构建

```rust
// server.rs 或 main.rs 启动路径
let aspectus_config = AspectusConfig::from_env()?;
let reqwest_client = reqwest::Client::builder()
    .timeout(Duration::from_millis(aspectus_config.timeout_ms))
    .build()?;
let aspectus = AspectusClient::with_reqwest(
    &aspectus_config.base_url,
    &aspectus_config.service_token,
    reqwest_client,
);

let state = Arc::new(AppState {
    tenant_manager,
    config,
    aspectus,
    tenant_cache: TenantCache::new(),
});
```

### 4.7 新增配置

```rust
pub struct AspectusConfig {
    pub base_url: String,        // ASPECTUS_BASE_URL，默认 http://localhost:3100
    pub service_token: String,   // ASPECTUS_SERVICE_TOKEN，必填
    pub timeout_ms: u64,         // ASPECTUS_TIMEOUT_MS，默认 2000
}

impl AspectusConfig {
    pub fn from_env() -> Result<Self, GatewayError>;
}
```

环境变量：
- `ASPECTUS_BASE_URL`（默认 `http://localhost:3100`）
- `ASPECTUS_SERVICE_TOKEN`（必填，无默认值）
- `ASPECTUS_TIMEOUT_MS`（默认 `2000`）

---

## 5. tenant crate 变更

### 5.1 `TenantManager` trait 签名

> **⚠️ Breaking change**：`create_session` 的第一个参数从 `&str tenant_id` 改为 `&TenantContext`。
> `TenantManager` 的唯一实现是 `TenantManagerImpl`（在 `tenant` crate 内部），
> 唯一消费方是 `api-gateway`。没有其他下游项目受影响。

```rust
#[async_trait]
pub trait TenantManager: Send + Sync {
    /// 创建 session。
    ///
    /// 从 `tenant_context` 中提取 tenant_id 和配额信息。
    /// 其他方法（send_message, list_sessions 等）仍接受 `&str tenant_id`，
    /// 因为这些操作在已有 session 上执行，不需要 Aspectus 上下文。
    async fn create_session(
        &self,
        tenant_context: &TenantContext,
        params: CreateSessionParams,
    ) -> Result<SessionInfo, TenantError>;

    // 其他方法不变...
}
```

### 5.2 `TenantManagerImpl` 实现

当前 `TenantManagerImpl::new()` 已通过 `HarnessConfig` 注入所有默认参数。不需要移除任何参数。仅需在 `create_session()` 内部将 `TenantContext` 传递给 `TenantRegistry::resolve_or_insert()`。

### 5.3 `TenantContext::from_introspect()` 实现细节

```rust
impl TenantContext {
    pub fn from_introspect(response: &IntrospectResponse) -> Result<Self, TenantError> {
        // 注：调用方（auth middleware）已检查 response.active，
        // 此处仅防御性检查。若 active 为 true 但 tenant_id 为 None，
        // 属于 Aspectus 的编程错误，使用 Internal 而非 IntrospectionInactive。
        let tenant_id = response.tenant_id.clone()
            .ok_or_else(|| TenantError::Internal {
                tenant_id: "unknown".into(),
                message: "active token has no tenant_id — Aspectus server bug".into(),
            })?;

        // 从 quotas HashMap 中提取 pandaria 配置
        let pandaria_quotas = response.quotas.as_ref()
            .and_then(|q| q.get("pandaria"))
            .ok_or_else(|| TenantError::TenantNotConfigured(tenant_id.clone()))?;

        let quotas = TenantQuota::from_aspectus_quotas(pandaria_quotas);

        Ok(Self {
            tenant_id,
            user_id: response.user_id.clone(),
            scopes: response.scope.as_ref()
                .map(|s| s.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
            quotas,
            cached_at: Instant::now(),
        })
    }
}
```

### 5.4 `TenantQuota::from_aspectus_quotas()` 设计

该方法为**可失败**（fallible）——当 `quotas.pandaria` 存在但值为非 JSON object（如字符串、数字、数组）时返回 `InvalidQuotasFormat`。

```rust
impl TenantQuota {
    pub fn from_aspectus_quotas(quotas: &serde_json::Value) -> Result<Self, TenantError> {
        let obj = quotas.as_object()
            .ok_or_else(|| TenantError::InvalidQuotasFormat(
                "quotas.pandaria must be a JSON object".into()
            ))?;

        Ok(Self {
            max_concurrent_sessions: extract_u32(obj, "max_concurrent_sessions", 10),
            max_tokens_per_day: extract_u64(obj, "max_tokens_per_day", 1_000_000),
            max_tool_calls_per_minute: extract_u32(obj, "max_tool_calls_per_minute", 60),
            cpu_time_budget_ms_per_day: extract_u64(obj, "cpu_time_budget_ms_per_day", 3_600_000),
        })
    }
}
```

> **设计理由**：`pandaria` key 存在但值类型错误 = 配置错误（硬错误，返回 `InvalidQuotasFormat`）。
> 个别字段缺失 = Aspectus schema 演进（软降级，使用默认值）。

### 5.5 无变更的模块

| 模块 | 说明 |
|---|---|
| `manager.rs` — Session 生命周期逻辑 | 核心逻辑不变，仅参数来源从 `Tenant` 变为 `TenantContext` |
| `supervisor.rs` — 并发槽位管理 | `acquire_slot()`/`release_slot()` 逻辑不变 |
| `meter.rs` — token 计量 | `record_tokens()` 逻辑不变 |
| `session_entry.rs` | 不变 |
| `events.rs` | 不变 |

---

## 6. 错误处理

### 6.1 Aspectus 不可用

- **场景**: Aspectus 服务宕机、网络超时、返回 5xx
- **行为**: auth middleware 返回 `503 Service Unavailable`
- **降级**: 无降级（用户决策：强制依赖 Aspectus，不保留 fallback）
- **重试**: `api-gateway` 中封装 `introspect_with_retry()`（指数退避，最多 2 次，100ms 基础延迟）。`aspectus-client` 自身不实现重试。
- **缓存**: auth middleware 使用 `TenantCache`（token → TenantContext，TTL 60s），减少 introspect 调用频率

### 6.2 租户未在 Aspectus 配置 pandaria 配额

- **场景**: introspect 返回 `active: true`，但 `quotas.pandaria` 为 `null` 或不存在
- **行为**: `TenantContext::from_introspect()` 返回 `TenantError::TenantNotConfigured`
- **用户可见**: auth middleware 返回 `403 Forbidden` + `"tenant not configured for pandaria"`

### 6.3 配额超限

- **行为不变**: `TenantSupervisor::acquire_slot()` 返回 `SessionLimitExceeded`
- **用户可见**: `429 Too Many Requests`

---

## 7. 测试策略

### 7.1 单元测试

| 测试目标 | 文件 | 说明 |
|---|---|---|
| `TenantContext::from_introspect()` | `tenant/src/context.rs` | 正常 JSON、缺失字段、空 quotas、无效 JSON |
| `TenantQuota::from_aspectus_quotas()` | `tenant/src/tenant.rs` | 完整字段、部分字段、无 pandaria key |
| `TenantRegistry::resolve_or_insert()` | `tenant/src/registry.rs` | 首次插入、缓存命中、过期刷新、并发插入 |
| `TenantRegistry::evict()` | `tenant/src/registry.rs` | 移除存在/不存在的租户 |
| auth middleware with mock AspectusClient | `api-gateway` | active token、inactive token、网络错误、超时 |

### 7.2 集成测试

| 测试 | 说明 |
|---|---|
| `e2e_aspectus_auth` | api-gateway 通过真实/模拟 Aspectus 验证 token 后创建 session |
| `e2e_aspectus_quotas` | Aspectus 返回不同配额值，验证 session 并发限制生效 |
| `e2e_aspectus_unavailable` | Aspectus 不可用时返回 503 |

### 7.3 Mock 策略

`aspectus-client` 目前无 trait 抽象（直接使用 `reqwest::Client`）。为支持单元测试，在 `api-gateway` 中使用 `wiremock` 启动本地 HTTP mock 模拟 `/introspect` 端点。

---

## 8. 迁移步骤

### Phase 1: 类型与接口准备（无行为变更）

1. 在 `tenant/src/context.rs` 中新增 `TenantContext`、`CachedContext`
2. `TenantQuota` 新增 `from_aspectus_quotas()` 方法（保留 `default()` 向后兼容）
3. `TenantRegistry` 新增 `resolve_or_insert(TenantContext)` 方法（保留 `register(Tenant)` deprecated）
4. `TenantSupervisor` 新增 `from_context()` 构造函数
5. 新增 `TenantError` 变体：`IntrospectionInactive`、`InvalidQuotasFormat`、`TenantNotConfigured`
6. `GatewayError` 新增变体：`ServiceUnavailable`、`Forbidden(String)`、`Internal(String)`
7. **此时编译通过，运行时行为不变**

### Phase 2: api-gateway 认证切换 + 清理本地注册（合并）

> **降低风险**：使用 Cargo feature flag `aspectus-auth` 门控新认证路径。
> Phase 1 在 `api-gateway/Cargo.toml` 中新增 `aspectus-auth` feature（默认关闭），
> 所有新代码放在 `#[cfg(feature = "aspectus-auth")]` 下。
> Phase 2 将 feature 设为默认开启，验证通过后移除 feature gate。

1. 新增 `aspectus-client` 路径依赖到 `api-gateway/Cargo.toml`（`path = "../../Aspectus/crates/aspectus-client"`），置于 `aspectus-auth` feature gate 下
2. 新增 `AspectusConfig` 和环境变量
3. 实现 `introspect_with_retry()` 封装（指数退避 2 次）
4. 实现 `TenantCache`（DashMap<String, CachedContext>，TTL 60s）
5. 重写 `auth_middleware` 使用 `AspectusClient` + 缓存
6. 路由 handler 从 `TenantId` extension 读取改为同时注入 `TenantId`（兼容 rate_limit）和 `TenantContext`
7. 移除 `verify_token()`、`base64_decode_urlsafe()`、`TokenPayload`
8. 移除 `TenantRegistry::register()`、`TenantRegistry::unregister()`
9. 移除 `Tenant` 结构体、`QuotaCheck` 枚举
10. 移除 `TenantQuota::default()`（硬编码默认值移入 `from_aspectus_quotas` 方法内部）
11. 移除 `AppConfig.auth_secret`
12. 移除 `main.rs` 中的 `register_dev_tenant()` 和 `generate_token()`

> **Phase 2 必须原子提交**：auth middleware 切换到 `TenantContext` 后，旧的 `register(Tenant)` 路径不再可用。合并不分阶段部署。

### Phase 3: 测试重写

1. 重写 `tests/e2e/common.rs` 测试辅助工厂函数（~6 个 builder），使用 `wiremock` 模拟 Aspectus `/introspect` 端点
2. 重写 `tests/e2e/e2e_*.rs`（~19 个测试文件），替换 `make_token()` + `registry.register()` 为 wiremock + `TenantContext`
3. 重写 `tenant/src/tests/` 中的单元测试，使用 `TenantContext` + `resolve_or_insert` 替代 `Tenant::new()`
4. 新增 `e2e_aspectus_auth`：验证 api-gateway 通过 wiremock Aspectus 验证 token
5. 新增 `e2e_aspectus_quotas`：验证不同配额值下 session 并发限制生效
6. 新增 `e2e_aspectus_unavailable`：验证 Aspectus 不可用时返回 503

### Phase 4: TUI 适配 + 文档

1. 更新 `crates/tui/src/dev_token.rs`：移除 HMAC token 生成，改为接受 Aspectus API Key（`pk_live_*`）
2. 移除 TUI 的 `hmac`/`sha2`/`base64` 依赖
3. 更新 `AGENTS.md` — 依赖方向图、模块边界
4. 更新 `README.md` — 核心能力、开发路线图
5. 更新 `VERSIONS.md` — 变更摘要
6. 更新 `docs/ecosystem.md` — Aspectus 集成状态 ✅
7. 新增 `docs/cookbook/` 本地开发指南（如何启动 Aspectus 进行本地开发）

### 本地开发引导

迁移后本地开发需要 Aspectus 服务。提供两种方式：

**A. 启动完整 Aspectus（推荐）**
```bash
cd ../Aspectus
docker compose up -d    # PostgreSQL
sqlx migrate run         # 建表
psql $DATABASE_URL -c "INSERT INTO tenants (id, name, quotas) VALUES ('dev', 'Development', '{}')"
psql $DATABASE_URL -c "INSERT INTO service_tokens (project, token_hash) VALUES ('pandaria', '$(echo -n 'dev-token' | sha256sum | cut -d' ' -f1)')"
cargo run -p aspectus-server
```

**B. wiremock 轻量模式（仅用于测试）**
```bash
# api-gateway 集成测试自动启动 wiremock
cargo test -p api-gateway --test e2e_aspectus_auth
```

---

## 9. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| Aspectus 不可用时整个 Pandaria 不可用 | **高** | `introspect_with_retry()` 指数退避重试；`TenantCache` 减少调用频率；运维侧监控 Aspectus 健康度 |
| `/introspect` 增加每个请求的延迟 | **中** | auth middleware 级 `TenantCache`（token→TenantContext，TTL 60s），后续请求无需调 Aspectus |
| 配额 JSON 格式变更导致解析失败 | **中** | `from_aspectus_quotas()` 对缺失字段静默降级；仅 `pandaria` key 缺失报硬错误 |
| E2E 测试套件重写工作量 | **高** | Phase 3 专门处理；使用 wiremock 模拟 Aspectus；保留旧测试作为回归参考直至全部迁移 |
| 本地开发需要 Aspectus 服务 | **中** | Phase 4 提供 docker-compose 快速启动指南 + wiremock 测试模式 |
| rate_limit_middleware 静默失效 | **高** | auth middleware 同时注入 `TenantId(String)` 和 `TenantContext`，rate limiter 继续使用 `TenantId` 提取 |
| `aspectus-client` 路径依赖跨 repo | **低** | 文档化 `path = "../../Aspectus/crates/aspectus-client"`；未来可发布 crates.io 版本 |
| TUI 客户端 HMAC token 生成失效 | **中** | Phase 4 更新 `dev_token.rs`；用户直接使用 Aspectus 签发的 `pk_live_*` API Key |

---

## 10. 参考

- [Aspectus AGENTS.md](../Aspectus/AGENTS.md) — ADR-001 (RFC 7662)、ADR-003 (配额管理)
- [Pandaria AGENTS.md](../AGENTS.md) — ADR-004 (Session 隔离)、ADR-005 (多租户基础能力)
- [ecosystem.md](../docs/ecosystem.md) — Aspectus 生态定位
- [aspectus-core introspect.rs](../Aspectus/crates/aspectus-core/src/introspect.rs) — `IntrospectResponse` 定义
- [aspectus-client lib.rs](../Aspectus/crates/aspectus-client/src/lib.rs) — `AspectusClient` API
