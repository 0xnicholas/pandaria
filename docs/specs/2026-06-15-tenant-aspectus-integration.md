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
pub struct TenantRegistry {
    tenants: DashMap<String, Arc<TenantSupervisor>>,
    cache_ttl: Duration,
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
pub enum TenantError {
    // 现有变体保留...
    TenantAlreadyExists(String),
    TenantNotFound(String),
    SessionLimitExceeded { tenant_id: String, limit: u32 },
    QuotaExceeded { tenant_id: String, reason: String },
    SessionNotFound(String),
    Internal(String),

    // 新增变体
    /// Aspectus introspection 返回 inactive.
    #[error("token introspection failed: inactive token")]
    IntrospectionInactive,
    /// quotas 字段缺失或格式错误.
    #[error("invalid quotas format in introspection response: {0}")]
    InvalidQuotasFormat(String),
    /// 租户未在 Aspectus 注册（introspect active=true 但 quotas.pandaria 为空）.
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

---

## 4. api-gateway 变更

### 4.1 auth middleware 重写

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

    let introspect = state.aspectus.introspect(token_str).await
        .map_err(|e| {
            tracing::warn!(error = %e, "aspectus introspection failed");
            GatewayError::ServiceUnavailable
        })?;

    if !introspect.active {
        return Err(GatewayError::Unauthorized);
    }

    let ctx = TenantContext::from_introspect(&introspect)
        .map_err(|e| {
            tracing::error!(error = %e, "invalid introspect response");
            GatewayError::Internal
        })?;

    req.extensions_mut().insert(ctx);
    // ...
}
```

### 4.2 AppState 新增字段

```rust
pub struct AppState {
    // 现有字段保留...
    pub tenant_manager: Arc<TenantManagerImpl>,
    pub config: AppConfig,

    // 新增
    pub aspectus: AspectusClient,
}
```

### 4.3 移除

- `auth.rs` 中的 `verify_token()`、`base64_decode_urlsafe()`、`TokenPayload`
- `AppConfig.auth_secret` 字段（或标记 deprecated）
- HMAC 相关依赖（若不再被其他模块使用）：`hmac`、`sha2` crate

### 4.4 新增配置

```rust
pub struct AspectusConfig {
    /// Aspectus 服务 URL（默认 http://localhost:3100）
    pub base_url: String,
    /// api-gateway 的 Service Token（用于调用 /introspect）
    pub service_token: String,
    /// introspection 超时（默认 2s）
    pub timeout_ms: u64,
    /// 租户缓存 TTL（默认 60s）
    pub cache_ttl_secs: u64,
}
```

环境变量：
- `ASPECTUS_BASE_URL`（默认 `http://localhost:3100`）
- `ASPECTUS_SERVICE_TOKEN`（必填，无默认值）
- `ASPECTUS_TIMEOUT_MS`（默认 `2000`）

---

## 5. tenant crate 变更

### 5.1 `TenantManager` trait 签名微调

```rust
#[async_trait]
pub trait TenantManager: Send + Sync {
    /// 创建 session（新增 tenant_context 参数）。
    async fn create_session(
        &self,
        tenant_context: &TenantContext,  // 新增
        params: CreateSessionParams,      // 保留
    ) -> Result<SessionInfo, TenantError>;

    /// 其他方法不变...
}
```

### 5.2 `TenantManagerImpl` 实现调整

```rust
impl TenantManagerImpl {
    /// 构造函数移除 tenant/provider/model 等「全局默认」参数。
    /// 这些参数现在从 Aspectus 或请求参数获取。
    pub fn new(
        registry: Arc<TenantRegistry>,
        store: Option<Arc<dyn SessionStore>>,
        hook_dispatcher_factory: HookDispatcherFactory,  // 新增：per-tenant hook 工厂
    ) -> Self;
}

/// Per-tenant hook dispatcher 工厂。
/// 因为不同租户可能有不同的 hook 策略（工具白名单、路径前缀等），
/// 每次创建 session 时需要根据 TenantContext 构建。
pub trait HookDispatcherFactory: Send + Sync {
    fn build(&self, ctx: &TenantContext) -> Arc<dyn HookDispatcher>;
}
```

### 5.3 无变更的模块

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
- **重试**: `AspectusClient` 内部实现指数退避重试（最多 2 次，100ms 基础延迟）

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

1. 在 `tenant/src/context.rs` 中新增 `TenantContext`
2. `TenantQuota` 新增 `from_aspectus_quotas()` 方法（保留 `default()` 向后兼容）
3. `TenantRegistry` 新增 `resolve_or_insert(TenantContext)` 方法（保留 `register(Tenant)` deprecated）
4. `TenantSupervisor` 新增 `from_context()` 构造函数
5. 新增 `TenantError` 变体
6. **此时编译通过，运行时行为不变**

### Phase 2: api-gateway 认证切换

1. 新增 `aspectus-client` 依赖到 `api-gateway/Cargo.toml`
2. 新增 `AspectusConfig` 和环境变量
3. 重写 `auth_middleware` 使用 `AspectusClient`
4. 移除 `verify_token`、`TokenPayload`、HMAC 相关代码
5. 路由 handler 从 `TenantId` extension 改为读取 `TenantContext`
6. `TenantManager::create_session()` 传入 `&TenantContext`

### Phase 3: 清理本地注册遗留

1. 移除 `TenantRegistry::register()`、`TenantRegistry::unregister()`
2. 移除 `Tenant` 结构体
3. 移除 `TenantQuota::default()`（硬编码默认值移入 `from_aspectus_quotas`）
4. 移除 `TenantManagerImpl::new()` 中的 `default_*` 参数
5. 移除 `AppConfig.auth_secret`

### Phase 4: 集成测试与文档

1. 新增 e2e 测试
2. 更新 `AGENTS.md`、`README.md`、`VERSIONS.md`
3. 更新 `docs/ecosystem.md` Aspectus 集成状态

---

## 9. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| Aspectus 不可用时整个 Pandaria 不可用 | 高 | `AspectusClient` 内建重试 + 超时；运维侧监控 Aspectus 健康度 |
| `/introspect` 增加每个请求的延迟 | 中 | 本地缓存 TenantContext（TTL 60s）；Aspectus p95 < 5ms 目标 |
| 配额 JSON 格式变更导致解析失败 | 低 | `from_aspectus_quotas()` 对缺失字段使用合理默认值 |
| 旧 HMAC token 客户端无法迁移 | 低 | 用户决策强制迁移，不保留兼容 |

---

## 10. 参考

- [Aspectus AGENTS.md](../Aspectus/AGENTS.md) — ADR-001 (RFC 7662)、ADR-003 (配额管理)
- [Pandaria AGENTS.md](../AGENTS.md) — ADR-004 (Session 隔离)、ADR-005 (多租户基础能力)
- [ecosystem.md](../docs/ecosystem.md) — Aspectus 生态定位
- [aspectus-core introspect.rs](../Aspectus/crates/aspectus-core/src/introspect.rs) — `IntrospectResponse` 定义
- [aspectus-client lib.rs](../Aspectus/crates/aspectus-client/src/lib.rs) — `AspectusClient` API
