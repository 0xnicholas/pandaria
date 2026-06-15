# Tenant → Aspectus Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor `tenant` crate to consume Aspectus as the single identity source; replace `api-gateway` HMAC auth with Aspectus introspection.

**Architecture:** `api-gateway` auth middleware calls Aspectus `/introspect` → builds `TenantContext` → injects into request. `TenantRegistry` becomes Aspectus adapter (resolve via `TenantContext`, TTL cache). `TenantManager` unchanged except `create_session` signature. Feature flag `aspectus-auth` gates new auth path until verified.

**Tech Stack:** Rust + tokio, `aspectus-client` (HTTP), `dashmap` (cache), `wiremock` (test)

**Spec:** `docs/specs/2026-06-15-tenant-aspectus-integration.md`

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `crates/tenant/src/context.rs` | **Create** | `TenantContext`, `CachedContext` types |
| `crates/tenant/src/tenant.rs` | **Modify** | Add `from_aspectus_quotas()`, mark `default()` deprecated |
| `crates/tenant/src/registry.rs` | **Modify** | Add `resolve_or_insert()`, `refresh()`; TTL cache |
| `crates/tenant/src/supervisor.rs` | **Modify** | Add `from_context()`; field `tenant: Tenant` → `tenant_id` + `quota` |
| `crates/tenant/src/error.rs` | **Modify** | New variants: `IntrospectionInactive`, `InvalidQuotasFormat`, `TenantNotConfigured` |
| `crates/tenant/src/manager.rs` | **Modify** | `create_session` signature: `&str` → `&TenantContext` |
| `crates/tenant/src/lib.rs` | **Modify** | Re-export `TenantContext` |
| `crates/api-gateway/Cargo.toml` | **Modify** | Add `aspectus-client` dep (path), feature `aspectus-auth` |
| `crates/api-gateway/src/error.rs` | **Modify** | Add `ServiceUnavailable`, `Forbidden`, `Internal` variants |
| `crates/api-gateway/src/config.rs` | **Modify** | Add `AspectusConfig` struct + `from_env()` |
| `crates/api-gateway/src/middleware/auth.rs` | **Modify** | Replace HMAC verify with AspectusClient + TenantCache |
| `crates/api-gateway/src/middleware/cache.rs` | **Create** | `TenantCache` with probabilistic eviction |
| `crates/api-gateway/src/middleware/mod.rs` | **Modify** | Re-export `TenantCache` |
| `crates/api-gateway/src/server.rs` | **Modify** | Build `AspectusClient` + `TenantCache` in AppState |
| `crates/api-gateway/src/routes/sessions.rs` | **Modify** | Extract `TenantContext` from extensions, pass to `create_session` |
| `crates/api-gateway/src/routes/messages.rs` | **Modify** | Extract `tenant_id` from `TenantContext` |
| `crates/api-gateway/src/main.rs` | **Modify** | Remove `register_dev_tenant()`, `generate_token()` |
| `crates/tenant/src/tests/` | **Modify** | Rewrite tests using `TenantContext` + `resolve_or_insert` |
| `crates/api-gateway/tests/e2e/common.rs` | **Modify** | Replace `make_token()`/`register()` with wiremock helpers |
| `crates/api-gateway/tests/e2e/e2e_aspectus_auth.rs` | **Create** | E2E: wiremock Aspectus → session creation |
| `crates/api-gateway/tests/e2e/e2e_aspectus_quotas.rs` | **Create** | E2E: quota enforcement from introspect |
| `crates/api-gateway/tests/e2e/e2e_aspectus_unavailable.rs` | **Create** | E2E: Aspectus unavailable → 503 |
| `crates/tui/src/dev_token.rs` | **Modify** | Remove HMAC token generation |
| `AGENTS.md`, `README.md`, `VERSIONS.md`, `docs/ecosystem.md` | **Modify** | Update integration status |

---

## Phase 1: Types & Interfaces (No Behavior Change)

### Task 1.1: Add `TenantContext` type

**Files:**
- Create: `crates/tenant/src/context.rs`
- Modify: `crates/tenant/src/lib.rs`

- [ ] **Step 1: Write `TenantContext` struct**

```rust
// crates/tenant/src/context.rs
use std::time::Instant;
use crate::error::TenantError;
use crate::tenant::TenantQuota;

/// Tenant context from Aspectus introspection response.
#[derive(Debug, Clone)]
pub struct TenantContext {
    pub tenant_id: String,
    pub user_id: Option<String>,
    pub scopes: Vec<String>,
    pub quotas: TenantQuota,
    pub cached_at: Instant,
}

impl TenantContext {
    /// Build from Aspectus IntrospectResponse.
    /// Caller must have already verified `response.active == true`.
    pub fn from_introspect(
        tenant_id: String,
        user_id: Option<String>,
        scope: Option<String>,
        quotas_json: Option<&serde_json::Value>,
    ) -> Result<Self, TenantError> {
        let pandaria_quotas = quotas_json
            .ok_or_else(|| TenantError::TenantNotConfigured(tenant_id.clone()))?;

        let quotas = TenantQuota::from_aspectus_quotas(pandaria_quotas)?;

        let scopes = scope
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        Ok(Self {
            tenant_id,
            user_id,
            scopes,
            quotas,
            cached_at: Instant::now(),
        })
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    pub fn is_stale(&self, ttl: std::time::Duration) -> bool {
        self.cached_at.elapsed() > ttl
    }
}
```

- [ ] **Step 2: Re-export from lib.rs**

Edit `crates/tenant/src/lib.rs`:
```rust
pub mod context;
pub use context::TenantContext;
```

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p tenant
```

- [ ] **Step 4: Commit**

```bash
git add crates/tenant/src/context.rs crates/tenant/src/lib.rs
git commit -m "feat(tenant): add TenantContext type"
```

### Task 1.2: Add `TenantQuota::from_aspectus_quotas()`

**Files:**
- Modify: `crates/tenant/src/tenant.rs`

- [ ] **Step 1: Add fallible from_aspectus_quotas**

```rust
// In TenantQuota impl block, add:
/// Parse quota from Aspectus quotas.pandaria JSON value.
/// Returns InvalidQuotasFormat if value is not a JSON object.
pub fn from_aspectus_quotas(
    quotas: &serde_json::Value,
) -> Result<Self, crate::error::TenantError> {
    use crate::error::TenantError;

    let obj = quotas.as_object().ok_or_else(|| {
        TenantError::InvalidQuotasFormat(
            "quotas.pandaria must be a JSON object".into(),
        )
    })?;

    Ok(Self {
        max_concurrent_sessions: extract_u32(obj, "max_concurrent_sessions", 10),
        max_tokens_per_day: extract_u64(obj, "max_tokens_per_day", 1_000_000),
        max_tool_calls_per_minute: extract_u32(obj, "max_tool_calls_per_minute", 60),
        cpu_time_budget_ms_per_day: extract_u64(
            obj,
            "cpu_time_budget_ms_per_day",
            3_600_000,
        ),
    })
}

fn extract_u32(obj: &serde_json::Map<String, serde_json::Value>, key: &str, default: u32) -> u32 {
    obj.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(default)
}

fn extract_u64(obj: &serde_json::Map<String, serde_json::Value>, key: &str, default: u64) -> u64 {
    obj.get(key)
        .and_then(|v| v.as_u64())
        .unwrap_or(default)
}
```

- [ ] **Step 2: Mark `TenantQuota::default()` deprecated**

```rust
#[deprecated(since = "0.2.0", note = "Use from_aspectus_quotas instead")]
impl Default for TenantQuota {
    fn default() -> Self { /* keep body */ }
}
```

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p tenant
```

- [ ] **Step 4: Commit**

```bash
git add crates/tenant/src/tenant.rs
git commit -m "feat(tenant): add TenantQuota::from_aspectus_quotas fallible parser"
```

### Task 1.3: Add `TenantError` new variants

**Files:**
- Modify: `crates/tenant/src/error.rs`

- [ ] **Step 1: Add three new variants**

```rust
// Add inside the TenantError enum (before the closing brace):
/// Aspectus introspection returned inactive token.
#[error("token introspection failed: inactive token")]
IntrospectionInactive,

/// quotas.pandaria JSON has invalid format.
#[error("invalid quotas format in introspection response: {0}")]
InvalidQuotasFormat(String),

/// Tenant exists in Aspectus but not configured for pandaria.
#[error("tenant {0} not configured for pandaria in Aspectus")]
TenantNotConfigured(String),
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p tenant
```

- [ ] **Step 3: Commit**

```bash
git add crates/tenant/src/error.rs
git commit -m "feat(tenant): add Aspectus-related error variants"
```

### Task 1.4: Add `TenantSupervisor::from_context()`

**Files:**
- Modify: `crates/tenant/src/supervisor.rs`

- [ ] **Step 1: Add constructor and refactor fields**

Read current `TenantSupervisor` struct. Add `from_context` constructor alongside existing `new(Tenant)`:

```rust
impl TenantSupervisor {
    /// Create supervisor from Aspectus TenantContext.
    pub fn from_context(ctx: &crate::context::TenantContext) -> Self {
        Self {
            tenant_id: ctx.tenant_id.clone(),
            quota: ctx.quotas,
            active_sessions: std::sync::atomic::AtomicU32::new(0),
            // ... copy other fields from existing new()
        }
    }
}
```

- [ ] **Step 2: Update field access from `self.tenant.id` → `self.tenant_id`**

Search for all usages of `self.tenant.id` and `self.tenant.quota` in `supervisor.rs`. Replace with `self.tenant_id` and `self.quota`.

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p tenant
```

- [ ] **Step 4: Commit**

```bash
git add crates/tenant/src/supervisor.rs
git commit -m "feat(tenant): add TenantSupervisor::from_context"
```

### Task 1.5: Add `TenantRegistry::resolve_or_insert()`

**Files:**
- Modify: `crates/tenant/src/registry.rs`

- [ ] **Step 1: Add resolve_or_insert method**

```rust
use std::time::Duration;
use crate::context::TenantContext;
use crate::supervisor::TenantSupervisor;

impl TenantRegistry {
    /// Resolve or create TenantSupervisor from TenantContext.
    /// Uses TTL cache (default 300s).
    pub fn resolve_or_insert(
        &self,
        ctx: &TenantContext,
    ) -> Result<Arc<TenantSupervisor>, TenantError> {
        // Check cache
        if let Some(existing) = self.tenants.get(&ctx.tenant_id) {
            if !existing.is_stale(self.cache_ttl) {
                return Ok(existing.clone());
            }
        }
        // Create new supervisor
        let supervisor = Arc::new(TenantSupervisor::from_context(ctx));
        self.tenants.insert(ctx.tenant_id.clone(), supervisor.clone());
        Ok(supervisor)
    }

    /// Force refresh a tenant's supervisor from new context.
    pub fn refresh(
        &self,
        ctx: &TenantContext,
    ) -> Result<Arc<TenantSupervisor>, TenantError> {
        let supervisor = Arc::new(TenantSupervisor::from_context(ctx));
        self.tenants.insert(ctx.tenant_id.clone(), supervisor.clone());
        Ok(supervisor)
    }

    /// Evict a tenant from cache.
    pub fn evict(&self, tenant_id: &str) -> Option<Arc<TenantSupervisor>> {
        self.tenants.remove(tenant_id).map(|(_, v)| v)
    }
}
```

- [ ] **Step 2: Add cache_ttl field to TenantRegistry**

```rust
pub struct TenantRegistry {
    tenants: DashMap<String, Arc<TenantSupervisor>>,
    cache_ttl: Duration,
}

impl TenantRegistry {
    pub fn new(cache_ttl: Duration) -> Self {
        Self { tenants: DashMap::new(), cache_ttl }
    }
    pub fn with_default_ttl() -> Self {
        Self::new(Duration::from_secs(300))
    }
}
```

- [ ] **Step 3: Update existing callers of `TenantRegistry::new()`**

`TenantRegistry::new()` now takes `Duration`. Update call sites:
- `TenantManagerImpl::new()` — pass `Duration::from_secs(300)`
- All tests — pass `Duration::from_secs(60)` for faster test eviction

- [ ] **Step 4: Verify compilation**

```bash
cargo check -p tenant -p api-gateway
```

- [ ] **Step 5: Commit**

```bash
git add crates/tenant/src/registry.rs crates/tenant/src/manager.rs crates/tenant/src/tests/
git commit -m "feat(tenant): add TenantRegistry::resolve_or_insert with TTL cache"
```

---

## Phase 2: api-gateway Auth Switch (Feature-Gated)

### Task 2.1: Add `aspectus-client` dependency and feature flag

**Files:**
- Modify: `crates/api-gateway/Cargo.toml`

- [ ] **Step 1: Add dependency with feature gate**

```toml
[features]
default = []
aspectus-auth = ["aspectus-client"]

[dependencies]
aspectus-client = { path = "../../Aspectus/crates/aspectus-client", optional = true }
```

- [ ] **Step 2: Verify compilation with feature enabled**

```bash
cargo check -p api-gateway --features aspectus-auth
```

- [ ] **Step 3: Commit**

```bash
git add crates/api-gateway/Cargo.toml
git commit -m "feat(api-gateway): add aspectus-client dependency behind feature flag"
```

### Task 2.2: Add `GatewayError` new variants

**Files:**
- Modify: `crates/api-gateway/src/error.rs`

- [ ] **Step 1: Add variants**

```rust
pub enum GatewayError {
    // ... existing variants ...
    /// Aspectus introspection failed or timed out → 503
    ServiceUnavailable,
    /// Tenant not configured for pandaria → 403
    Forbidden(String),
    /// Internal error → 500
    Internal(String),
}
```

- [ ] **Step 2: Add IntoResponse mappings**

Add impl blocks for the new variants returning appropriate status codes and bodies.

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p api-gateway
```

- [ ] **Step 4: Commit**

```bash
git add crates/api-gateway/src/error.rs
git commit -m "feat(api-gateway): add ServiceUnavailable, Forbidden, Internal error variants"
```

### Task 2.3: Add `AspectusConfig`

**Files:**
- Modify: `crates/api-gateway/src/config.rs`

- [ ] **Step 1: Add config struct**

```rust
#[cfg(feature = "aspectus-auth")]
#[derive(Debug, Clone)]
pub struct AspectusConfig {
    pub base_url: String,
    pub service_token: String,
    pub timeout_ms: u64,
}

#[cfg(feature = "aspectus-auth")]
impl AspectusConfig {
    pub fn from_env() -> Result<Self, crate::error::GatewayError> {
        Ok(Self {
            base_url: std::env::var("ASPECTUS_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:3100".into()),
            service_token: std::env::var("ASPECTUS_SERVICE_TOKEN")
                .map_err(|_| crate::error::GatewayError::Internal(
                    "ASPECTUS_SERVICE_TOKEN not set".into()
                ))?,
            timeout_ms: std::env::var("ASPECTUS_TIMEOUT_MS")
                .unwrap_or_else(|_| "2000".into())
                .parse()
                .unwrap_or(2000),
        })
    }
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p api-gateway --features aspectus-auth
```

- [ ] **Step 3: Commit**

```bash
git add crates/api-gateway/src/config.rs
git commit -m "feat(api-gateway): add AspectusConfig"
```

### Task 2.4: Add `TenantCache`

**Files:**
- Create: `crates/api-gateway/src/middleware/cache.rs`
- Modify: `crates/api-gateway/src/middleware/mod.rs`

- [ ] **Step 1: Implement TenantCache**

```rust
// crates/api-gateway/src/middleware/cache.rs
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tenant::TenantContext;

struct CacheEntry {
    ctx: TenantContext,
    inserted_at: Instant,
}

pub struct TenantCache {
    entries: DashMap<String, CacheEntry>,
    counter: AtomicU64,
}

impl TenantCache {
    pub fn new() -> Self {
        Self { entries: DashMap::new(), counter: AtomicU64::new(0) }
    }

    pub fn get(&self, token: &str) -> Option<TenantContext> {
        // Probabilistic cleanup every 1024 checks
        if self.counter.fetch_add(1, Ordering::Relaxed) % 1024 == 0 {
            let now = Instant::now();
            self.entries.retain(|_, v| {
                now.duration_since(v.inserted_at) < Duration::from_secs(300)
            });
        }
        self.entries.get(token)
            .filter(|e| e.inserted_at.elapsed() < Duration::from_secs(60))
            .map(|e| e.ctx.clone())
    }

    pub fn insert(&self, token: String, ctx: TenantContext) {
        self.entries.insert(token, CacheEntry { ctx, inserted_at: Instant::now() });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
```

- [ ] **Step 2: Re-export from middleware mod.rs**

```rust
#[cfg(feature = "aspectus-auth")]
mod cache;
#[cfg(feature = "aspectus-auth")]
pub use cache::TenantCache;
```

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p api-gateway --features aspectus-auth
```

- [ ] **Step 4: Commit**

```bash
git add crates/api-gateway/src/middleware/cache.rs crates/api-gateway/src/middleware/mod.rs
git commit -m "feat(api-gateway): add TenantCache with probabilistic eviction"
```

### Task 2.5: Rewrite auth middleware

**Files:**
- Modify: `crates/api-gateway/src/middleware/auth.rs`

- [ ] **Step 1: Add Aspectus auth path (feature-gated)**

Keep existing HMAC auth. Add `#[cfg(feature = "aspectus-auth")]` alternative:

```rust
#[cfg(feature = "aspectus-auth")]
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, GatewayError> {
    if req.uri().path() == "/healthz" {
        return Ok(next.run(req).await);
    }

    let token_str = extract_bearer_token(&req)?;

    // Check cache first
    if let Some(ctx) = state.tenant_cache.get(token_str) {
        req.extensions_mut().insert(ctx.tenant_id.clone()); // TenantId for rate_limit
        req.extensions_mut().insert(ctx.clone());           // TenantContext for handlers
        let span = make_span(&req, &ctx.tenant_id);
        return Ok(async move { next.run(req).await }.instrument(span).await);
    }

    // Introspect with retry
    let introspect = introspect_with_retry(&state.aspectus, token_str).await
        .map_err(|e| {
            tracing::warn!(error = %e, "aspectus introspection failed");
            GatewayError::ServiceUnavailable
        })?;

    if !introspect.active {
        return Err(GatewayError::Unauthorized);
    }

    let ctx = tenant::TenantContext::from_introspect(
        introspect.tenant_id.ok_or(GatewayError::Unauthorized)?,
        introspect.user_id,
        introspect.scope,
        introspect.quotas.as_ref().and_then(|q| q.get("pandaria")),
    ).map_err(|e| match &e {
        tenant::TenantError::TenantNotConfigured(_) =>
            GatewayError::Forbidden("tenant not configured for pandaria".into()),
        _ => GatewayError::Internal(e.to_string()),
    })?;

    // Cache and inject
    state.tenant_cache.insert(token_str.to_string(), ctx.clone());
    req.extensions_mut().insert(ctx.tenant_id.clone());
    req.extensions_mut().insert(ctx.clone());

    let span = make_span(&req, &ctx.tenant_id);
    Ok(async move { next.run(req).await }.instrument(span).await)
}

#[cfg(feature = "aspectus-auth")]
async fn introspect_with_retry(
    client: &aspectus_client::AspectusClient,
    token: &str,
) -> Result<aspectus_core::introspect::IntrospectResponse, aspectus_client::ClientError> {
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

- [ ] **Step 2: Wrap existing HMAC auth with `#[cfg(not(feature = "aspectus-auth"))]`**

- [ ] **Step 3: Verify compilation both ways**

```bash
cargo check -p api-gateway
cargo check -p api-gateway --features aspectus-auth
```

- [ ] **Step 4: Commit**

```bash
git add crates/api-gateway/src/middleware/auth.rs
git commit -m "feat(api-gateway): add Aspectus auth middleware behind feature flag"
```

### Task 2.6: Update AppState and server startup

**Files:**
- Modify: `crates/api-gateway/src/server.rs`

- [ ] **Step 1: Add aspectus fields to AppState**

```rust
#[cfg(feature = "aspectus-auth")]
use crate::middleware::TenantCache;

pub struct AppState {
    // ... existing fields ...
    #[cfg(feature = "aspectus-auth")]
    pub aspectus: aspectus_client::AspectusClient,
    #[cfg(feature = "aspectus-auth")]
    pub tenant_cache: TenantCache,
}
```

- [ ] **Step 2: Build aspectus client in server startup**

```rust
#[cfg(feature = "aspectus-auth")]
{
    let aspectus_config = crate::config::AspectusConfig::from_env()?;
    let reqwest_client = reqwest::Client::builder()
        .timeout(Duration::from_millis(aspectus_config.timeout_ms))
        .build()
        .map_err(|e| GatewayError::Internal(e.to_string()))?;
    let aspectus = aspectus_client::AspectusClient::with_reqwest(
        &aspectus_config.base_url,
        &aspectus_config.service_token,
        reqwest_client,
    );
    // ... pass to AppState
}
```

- [ ] **Step 3: Verify compilation both ways**

```bash
cargo check -p api-gateway
cargo check -p api-gateway --features aspectus-auth
```

- [ ] **Step 4: Commit**

```bash
git add crates/api-gateway/src/server.rs
git commit -m "feat(api-gateway): add AspectusClient + TenantCache to AppState"
```

### Task 2.7: Update route handlers

**Files:**
- Modify: `crates/api-gateway/src/routes/sessions.rs`
- Modify: `crates/api-gateway/src/routes/messages.rs`

- [ ] **Step 1: Extract TenantContext in session create**

```rust
#[cfg(feature = "aspectus-auth")]
let tenant_ctx = req.extensions().get::<tenant::TenantContext>()
    .ok_or(GatewayError::Unauthorized)?;
#[cfg(not(feature = "aspectus-auth"))]
let tenant_id = req.extensions().get::<TenantId>()
    .map(|t| t.0.clone())
    .ok_or(GatewayError::Unauthorized)?;

#[cfg(feature = "aspectus-auth")]
let session = state.tenant_manager.create_session(tenant_ctx, params).await?;
#[cfg(not(feature = "aspectus-auth"))]
let session = state.tenant_manager.create_session(&tenant_id, params).await?;
```

- [ ] **Step 2: Update `TenantManager::create_session` signature**

In `crates/tenant/src/manager.rs`:
```rust
#[cfg(feature = "aspectus-auth")]
async fn create_session(&self, tenant_context: &TenantContext, params: CreateSessionParams) -> ...;
```

For Phase 2, use compile-time switching. This requires adding `aspectus-auth` feature to `tenant` crate's `Cargo.toml` too:

```toml
# crates/tenant/Cargo.toml
[features]
aspectus-auth = []
```

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p api-gateway --features aspectus-auth
```

- [ ] **Step 4: Commit**

```bash
git add crates/api-gateway/src/routes/sessions.rs crates/api-gateway/src/routes/messages.rs crates/tenant/src/manager.rs crates/tenant/Cargo.toml
git commit -m "feat(api-gateway): update route handlers for TenantContext"
```

### Task 2.8: Remove legacy HMAC code and local registration

**Files:**
- Modify: `crates/api-gateway/src/middleware/auth.rs` — remove `#[cfg(not(...))]` path
- Modify: `crates/api-gateway/src/main.rs` — remove `register_dev_tenant()`, `generate_token()`
- Modify: `crates/tenant/src/registry.rs` — remove `register()`, `unregister()`
- Modify: `crates/tenant/src/tenant.rs` — remove `Tenant` struct, `TenantQuota::default()`, `QuotaCheck`

- [ ] **Step 1: Enable feature by default**

```toml
# crates/api-gateway/Cargo.toml
[features]
default = ["aspectus-auth"]
```

- [ ] **Step 2: Remove all `#[cfg(not(feature = "aspectus-auth"))]` blocks**

- [ ] **Step 3: Remove legacy types**

- [ ] **Step 4: Verify compilation**

```bash
cargo check -p tenant -p api-gateway
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat!: remove legacy HMAC auth and local tenant registration"
```

---

## Phase 3: Test Rewrite

### Task 3.1: Update E2E test helpers

**Files:**
- Modify: `crates/api-gateway/tests/e2e/common.rs`

- [ ] **Step 1: Add wiremock Aspectus helper**

```rust
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path};

pub struct AspectusMock {
    server: MockServer,
}

impl AspectusMock {
    pub async fn start() -> Self {
        Self { server: MockServer::start().await }
    }

    pub fn base_url(&self) -> String { self.server.uri() }

    /// Mock successful introspection
    pub async fn mock_active_tenant(&self, tenant_id: &str) {
        Mock::given(method("POST"))
            .and(path("/introspect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({
                    "active": true,
                    "tenant_id": tenant_id,
                    "user_id": "user-1",
                    "scope": "pandaria:session:create pandaria:session:read",
                    "quotas": {
                        "pandaria": {
                            "max_concurrent_sessions": 10,
                            "max_tokens_per_day": 1000000,
                            "max_tool_calls_per_minute": 60,
                            "cpu_time_budget_ms_per_day": 3600000
                        }
                    }
                })
            ))
            .mount(&self.server)
            .await;
    }

    /// Mock inactive token
    pub async fn mock_inactive(&self) {
        Mock::given(method("POST"))
            .and(path("/introspect"))
            .respond_with(ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"active": false})))
            .mount(&self.server)
            .await;
    }
}
```

- [ ] **Step 2: Add `build_test_app_with_aspectus` factory**

```rust
pub async fn build_test_app_with_aspectus(
    registry: Arc<TenantRegistry>,
    aspectus_url: String,
) -> (Router, Arc<AppState>) {
    let aspectus = AspectusClient::new(aspectus_url, "test-service-token");
    let state = Arc::new(AppState {
        tenant_manager: Arc::new(TenantManagerImpl::new(registry, /* ... */)),
        aspectus,
        tenant_cache: TenantCache::new(),
        // ... other fields
    });
    (build_router(state.clone()), state)
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/api-gateway/tests/e2e/common.rs
git commit -m "test(api-gateway): add wiremock Aspectus helpers for E2E tests"
```

### Task 3.2: Write new E2E tests

**Files:**
- Create: `crates/api-gateway/tests/e2e/e2e_aspectus_auth.rs`
- Create: `crates/api-gateway/tests/e2e/e2e_aspectus_quotas.rs`
- Create: `crates/api-gateway/tests/e2e/e2e_aspectus_unavailable.rs`

- [ ] **Step 1: e2e_aspectus_auth — happy path**

```rust
#[tokio::test]
async fn test_create_session_with_aspectus_auth() {
    let aspectus = AspectusMock::start().await;
    aspectus.mock_active_tenant("test-tenant").await;

    let registry = Arc::new(TenantRegistry::with_default_ttl());
    let (router, _state) = build_test_app_with_aspectus(registry, aspectus.base_url()).await;

    let resp = router
        .oneshot(
            Request::post("/api/v1/sessions")
                .header("Authorization", "Bearer pk_live_test123")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title":"test"}"#))
                .unwrap()
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), 201);
}
```

- [ ] **Step 2: e2e_aspectus_quotas — quota enforcement**

Test that `max_concurrent_sessions: 1` in introspect response limits session creation to 1.

- [ ] **Step 3: e2e_aspectus_unavailable — 503**

Test that when Aspectus is unreachable, api-gateway returns 503.

- [ ] **Step 4: Run tests**

```bash
cargo test -p api-gateway --test e2e_aspectus_auth
cargo test -p api-gateway --test e2e_aspectus_quotas
cargo test -p api-gateway --test e2e_aspectus_unavailable
```

- [ ] **Step 5: Commit**

```bash
git add crates/api-gateway/tests/e2e/
git commit -m "test(api-gateway): add Aspectus auth E2E tests"
```

### Task 3.3: Update tenant unit tests

**Files:**
- Modify: `crates/tenant/src/tests/` (registry, quota, supervisor)

- [ ] **Step 1: Rewrite tests to use `TenantContext` + `resolve_or_insert`**

Replace all `Tenant::new()` and `registry.register()` calls with `TenantContext` construction and `resolve_or_insert()`.

- [ ] **Step 2: Run tests**

```bash
cargo test -p tenant
```

- [ ] **Step 3: Commit**

```bash
git add crates/tenant/src/tests/
git commit -m "test(tenant): rewrite tests for TenantContext + resolve_or_insert"
```

---

## Phase 4: TUI & Docs

### Task 4.1: Update TUI dev_token

**Files:**
- Modify: `crates/tui/src/dev_token.rs`

- [ ] **Step 1: Remove HMAC token generation**

Replace `dev_token.rs` content with documentation instructing users to use Aspectus-issued API keys (`pk_live_*`).

- [ ] **Step 2: Remove HMAC deps from TUI Cargo.toml**

Remove `hmac`, `sha2`, `base64` dependencies.

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p tui
```

- [ ] **Step 4: Commit**

```bash
git add crates/tui/
git commit -m "feat(tui): remove HMAC token generation, use Aspectus API keys"
```

### Task 4.2: Update project docs

**Files:**
- Modify: `AGENTS.md`, `README.md`, `VERSIONS.md`, `docs/ecosystem.md`

- [ ] **Step 1: Update AGENTS.md**

- Dependency diagram: add `aspectus-client` arrow from api-gateway
- 当前状态: add "Aspectus 集成" row (✅)

- [ ] **Step 2: Update README.md**

- 核心能力: add "统一身份（Aspectus）" row
- 技术架构图: add Aspectus as external service

- [ ] **Step 3: Update VERSIONS.md**

- Add v0.2.0 变更摘要 entry for Aspectus integration

- [ ] **Step 4: Update ecosystem.md**

- Mark "全部 → Aspectus" integration status as ✅

- [ ] **Step 5: Commit**

```bash
git add AGENTS.md README.md VERSIONS.md docs/ecosystem.md
git commit -m "docs: update for Aspectus integration"
```

---

## Verification

After all phases:

```bash
# Full workspace check
cargo check --workspace --all-features

# All tests
cargo test --workspace --lib
cargo test -p api-gateway --test e2e_aspectus_auth
cargo test -p api-gateway --test e2e_aspectus_quotas
cargo test -p api-gateway --test e2e_aspectus_unavailable

# Clippy
cargo clippy --workspace --all-features -- -D warnings
```
