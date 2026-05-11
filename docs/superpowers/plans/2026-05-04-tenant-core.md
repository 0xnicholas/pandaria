# Tenant Core (`crates/tenant/`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `crates/tenant/` — the per-tenant registry, quota management, session tracking, and resource metering subsystem that makes multi-tenancy real instead of just a string field.

**Architecture:** TenantRegistry is a global singleton holding per-tenant TenantSupervisors. Each TenantSupervisor tracks active sessions, token/CPU/tool-call usage via sliding-window meters, and enforces per-tenant quotas. Quota enforcement is implemented as Extensions (TenantQuotaExtension, TenantTokenMeterExtension) that plug into the existing HookRouter, replacing the current global RateLimitExtension.

**Tech Stack:** Rust 2024, tokio, dashmap (concurrent HashMap), thiserror, async-trait. Reuses existing agent-core types and extensions hook system.

---

## Design Decisions (Confirmed)

| # | Decision | Rationale |
|---|----------|-----------|
| 1 | **Unregistered tenants are blocked by default.** `TenantQuotaExtension` returns `Block` for unknown `tenant_id`. A config flag `allow_unknown: bool` (default `false`) allows opt-in bypass. | Defense in depth — every layer enforces its own boundary. Do not assume upstream auth is infallible. |
| 2 | **Token budget is enforced at the api-gateway layer, not inline in the Extension.** `TenantTokenMeterExtension` records usage (observational). `TenantQuotaExtension` only enforces tool-call rate limits (blocking). Token budget `check_quota(TokenUsage)` is called by the caller (api-gateway or session factory) before accepting new prompts. | The Extension system lacks a working `on_before_provider_request` hook (marked TODO in HookRouter). Attempting to enforce token budget inside a chain hook (`on_context`) would distort the agent loop. Honest architecture boundary. |
| 3 | **Session leak protection via RAII guard.** `reserve_session()` returns `SessionGuard` which auto-releases on `Drop`. | Manual `reserve + release` is error-prone across panic/cancellation paths. RAII is idiomatic Rust. |
| 4 | **Meter memory bounded by capacity-triggered truncation.** `SlidingWindowMeter` truncates the oldest 50% of entries when `len() > MAX_ENTRIES` (10,000). No background prune task. | Background tasks add lifecycle complexity. Capacity trigger is deterministic and sufficient for MVP. |
| 5 | **Remove unused `TenantError::QuotaExceeded`.** Keep only the three specific limit errors. | Dead code elimination. |
| 6 | **Extension registration order:** `TenantQuotaExtension` must be registered *before* `TenantTokenMeterExtension`. | If meter runs before quota, blocked tool-calls are still counted. |

---

## File Structure

### New Files (tenant crate)

| File | Responsibility |
|---|---|
| `crates/tenant/Cargo.toml` | Crate manifest, depends on agent-core, llm-client, extensions, dashmap |
| `crates/tenant/src/lib.rs` | Public exports |
| `crates/tenant/src/error.rs` | `TenantError` enum (thiserror) |
| `crates/tenant/src/tenant.rs` | `Tenant` struct, `TenantQuota`, `QuotaCheck` |
| `crates/tenant/src/registry.rs` | `TenantRegistry` — global DashMap of tenant_id → Arc<TenantSupervisor> |
| `crates/tenant/src/supervisor.rs` | `TenantSupervisor` — per-tenant session tracking, quota enforcement, metering |
| `crates/tenant/src/meter.rs` | `SlidingWindowMeter` — generic sliding-window counter for tokens/tool-calls/CPU-time |
| `crates/tenant/src/extensions/mod.rs` | Extension sub-module exports |
| `crates/tenant/src/extensions/quota.rs` | `TenantQuotaExtension` — per-tenant tool-call rate limit + token budget check (blocking hook) |
| `crates/tenant/src/extensions/token_meter.rs` | `TenantTokenMeterExtension` — records usage on turn_end (observational hook) |
| `crates/tenant/README.md` | Crate responsibility, public API, boundary docs |
| `crates/tenant/tests/registry.rs` | TenantRegistry concurrency + lifecycle tests |
| `crates/tenant/tests/quota.rs` | Quota enforcement tests |
| `crates/tenant/tests/supervisor.rs` | TenantSupervisor session tracking + metering tests |

### Modified Files (agent-core)

| File | Change |
|---|---|
| `crates/agent-core/src/context.rs` | Add `usage: llm_client::Usage` field to `TurnEndCtx` |
| `crates/agent-core/src/loop.rs` | Pass `usage` when constructing `TurnEndCtx` at lines ~313 and ~490 |

### Modified Files (workspace)

| File | Change |
|---|---|
| `Cargo.toml` | Add `"crates/tenant"` to workspace members |

---

## Task Breakdown

### Task 1: Scaffold the tenant crate

**Files:**
- Create: `crates/tenant/Cargo.toml`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Create crate manifest**

`crates/tenant/Cargo.toml`:
```toml
[package]
name = "tenant"
version = "0.1.0"
edition = "2024"

[dependencies]
agent-core = { path = "../agent-core" }
llm-client = { path = "../llm-client" }
extensions = { path = "../extensions" }
tokio = { workspace = true }
async-trait = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
dashmap = "6"

[dev-dependencies]
tokio = { workspace = true, features = ["rt-multi-thread", "macros"] }
tracing-subscriber = { workspace = true }
serde_json = { workspace = true }
tokio-util = { workspace = true }
```

- [ ] **Step 2: Add to workspace**

Modify `Cargo.toml` workspace members to include `"crates/tenant"`.

- [ ] **Step 3: Create placeholder lib.rs**

`crates/tenant/src/lib.rs`:
```rust
//! Tenant management: per-tenant registry, quota enforcement, and resource metering.

pub mod error;
pub mod extensions;
pub mod meter;
pub mod registry;
pub mod supervisor;
pub mod tenant;

pub use error::TenantError;
pub use registry::TenantRegistry;
pub use supervisor::{QuotaStatus, TenantSupervisor};
pub use tenant::{QuotaCheck, Tenant, TenantQuota};
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p tenant`
Expected: succeeds (empty crate compiles)

---

### Task 2: Define error types

**Files:**
- Create: `crates/tenant/src/error.rs`

- [ ] **Step 1: Write the failing test**

`crates/tenant/tests/registry.rs` (placeholder):
```rust
#[test]
fn test_tenant_error_display() {
    let err = tenant::TenantError::TenantNotFound("t1".to_string());
    assert!(err.to_string().contains("t1"));
}
```

Run: `cargo test -p tenant test_tenant_error_display`
Expected: FAIL — TenantError not defined

- [ ] **Step 2: Implement TenantError**

`crates/tenant/src/error.rs`:
```rust
use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum TenantError {
    #[error("tenant not found: {0}")]
    TenantNotFound(String),

    #[error("tenant already registered: {0}")]
    TenantAlreadyExists(String),

    #[error("session limit exceeded for tenant {tenant_id}: max {max}, current {current}")]
    SessionLimitExceeded {
        tenant_id: String,
        max: u32,
        current: u32,
    },

    #[error("token budget exceeded for tenant {tenant_id}: consumed {consumed}, budget {budget}")]
    TokenBudgetExceeded {
        tenant_id: String,
        consumed: u64,
        budget: u64,
    },

    #[error("tool call rate limit exceeded for tenant {tenant_id}: {calls} calls in window")]
    ToolCallRateLimitExceeded {
        tenant_id: String,
        calls: usize,
    },
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p tenant test_tenant_error_display`
Expected: PASS

---

### Task 3: Define Tenant and TenantQuota

**Files:**
- Create: `crates/tenant/src/tenant.rs`

- [ ] **Step 1: Write the failing test**

`crates/tenant/tests/quota.rs`:
```rust
use tenant::{Tenant, TenantQuota};

#[test]
fn test_tenant_quota_defaults() {
    let quota = TenantQuota::default();
    assert_eq!(quota.max_concurrent_sessions, 10);
    assert_eq!(quota.max_tokens_per_day, 1_000_000);
}

#[test]
fn test_tenant_creation() {
    let tenant = Tenant::new("t1", TenantQuota::default());
    assert_eq!(tenant.id, "t1");
}
```

Run: `cargo test -p tenant`
Expected: FAIL — Tenant, TenantQuota not defined

- [ ] **Step 2: Implement Tenant and TenantQuota**

`crates/tenant/src/tenant.rs`:
```rust
/// Resource quota configuration for a single tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantQuota {
    /// Maximum number of concurrently active sessions.
    pub max_concurrent_sessions: u32,
    /// Maximum LLM tokens (input + output) per day.
    pub max_tokens_per_day: u64,
    /// Maximum tool calls per minute.
    pub max_tool_calls_per_minute: u32,
    /// CPU time budget in milliseconds per day (wall-clock proxy).
    pub cpu_time_budget_ms_per_day: u64,
}

impl Default for TenantQuota {
    fn default() -> Self {
        Self {
            max_concurrent_sessions: 10,
            max_tokens_per_day: 1_000_000,
            max_tool_calls_per_minute: 60,
            cpu_time_budget_ms_per_day: 3_600_000, // 1 hour
        }
    }
}

/// A registered tenant with its quota.
#[derive(Debug, Clone)]
pub struct Tenant {
    pub id: String,
    pub quota: TenantQuota,
}

impl Tenant {
    pub fn new(id: impl Into<String>, quota: TenantQuota) -> Self {
        Self {
            id: id.into(),
            quota,
        }
    }
}

/// Type of quota check requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaCheck {
    SessionCreation,
    ToolCall,
    TokenUsage { input: u64, output: u64 },
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p tenant`
Expected: PASS

---

### Task 4: Implement SlidingWindowMeter

**Files:**
- Create: `crates/tenant/src/meter.rs`

- [ ] **Step 1: Write the failing test**

`crates/tenant/tests/supervisor.rs`:
```rust
use tenant::meter::SlidingWindowMeter;
use std::time::{Duration, Instant};

#[test]
fn test_sliding_window_count() {
    let meter = SlidingWindowMeter::new(Duration::from_secs(60));
    meter.record(100);
    meter.record(200);
    assert_eq!(meter.sum(), 300);
    assert_eq!(meter.count(), 2);
}

#[tokio::test]
async fn test_sliding_window_expiration() {
    let meter = SlidingWindowMeter::new(Duration::from_millis(50));
    meter.record(100);
    assert_eq!(meter.sum(), 100);
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(meter.sum(), 0);
}
```

Run: `cargo test -p tenant`
Expected: FAIL — SlidingWindowMeter not defined

- [ ] **Step 2: Implement SlidingWindowMeter**

`crates/tenant/src/meter.rs`:
```rust
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Thread-safe sliding-window meter for counting events or summing values.
pub struct SlidingWindowMeter {
    window: Duration,
    entries: Mutex<Vec<(Instant, u64)>>,
}

impl SlidingWindowMeter {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            entries: Mutex::new(Vec::new()),
        }
    }

    const MAX_ENTRIES: usize = 10_000;

    /// Record a value at the current time.
    pub fn record(&self, value: u64) {
        let mut entries = self.entries.lock().expect("meter mutex poisoned");
        let now = Instant::now();
        Self::prune(&mut entries, now, self.window);
        entries.push((now, value));

        // Capacity-triggered truncation: oldest 50% dropped when over limit.
        if entries.len() > Self::MAX_ENTRIES {
            let cutoff = entries.len() / 2;
            entries.drain(..cutoff);
        }
    }

    /// Sum of all values in the current window.
    pub fn sum(&self) -> u64 {
        let mut entries = self.entries.lock().expect("meter mutex poisoned");
        let now = Instant::now();
        Self::prune(&mut entries, now, self.window);
        entries.iter().map(|(_, v)| v).sum()
    }

    /// Count of entries in the current window.
    pub fn count(&self) -> usize {
        let mut entries = self.entries.lock().expect("meter mutex poisoned");
        let now = Instant::now();
        Self::prune(&mut entries, now, self.window);
        entries.len()
    }

    fn prune(entries: &mut Vec<(Instant, u64)>, now: Instant, window: Duration) {
        entries.retain(|(t, _)| now.duration_since(*t) < window);
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p tenant`
Expected: PASS

---

### Task 5: Implement TenantSupervisor

**Files:**
- Create: `crates/tenant/src/supervisor.rs`

- [ ] **Step 1: Write the failing test**

`crates/tenant/tests/supervisor.rs`:
```rust
use tenant::{Tenant, TenantQuota, TenantSupervisor, QuotaCheck};
use std::sync::Arc;

#[test]
fn test_supervisor_session_tracking() {
    let tenant = Tenant::new("t1", TenantQuota {
        max_concurrent_sessions: 2,
        ..TenantQuota::default()
    });
    let supervisor = TenantSupervisor::new(tenant);

    // Reserve 2 sessions (SessionGuard auto-releases on Drop)
    let _guard1 = supervisor.reserve_session().unwrap();
    let _guard2 = supervisor.reserve_session().unwrap();

    // 3rd should fail
    let err = supervisor.reserve_session().unwrap_err();
    assert!(matches!(err, tenant::TenantError::SessionLimitExceeded { .. }));

    // Drop one guard, then reserve should succeed
    drop(_guard1);
    let _guard3 = supervisor.reserve_session().unwrap();
}

#[test]
fn test_supervisor_token_metering() {
    let tenant = Tenant::new("t1", TenantQuota {
        max_tokens_per_day: 100,
        ..TenantQuota::default()
    });
    let supervisor = TenantSupervisor::new(tenant);

    supervisor.record_usage(&llm_client::Usage {
        input_tokens: 30,
        output_tokens: 20,
        total_tokens: 50,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });

    let status = supervisor.quota_status();
    assert_eq!(status.tokens_consumed, 50);

    // Exceed budget
    let err = supervisor.check_quota(QuotaCheck::TokenUsage { input: 30, output: 30 }).unwrap_err();
    assert!(matches!(err, tenant::TenantError::TokenBudgetExceeded { .. }));
}
```

Run: `cargo test -p tenant`
Expected: FAIL — TenantSupervisor not defined

- [ ] **Step 2: Implement TenantSupervisor**

`crates/tenant/src/supervisor.rs`:
```rust
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::error::TenantError;
use crate::meter::SlidingWindowMeter;
use crate::tenant::{QuotaCheck, Tenant, TenantQuota};

/// Per-tenant resource supervisor. Tracks active sessions and usage meters.
pub struct TenantSupervisor {
    tenant: Tenant,
    active_sessions: AtomicUsize,
    token_meter: SlidingWindowMeter,      // 24h window
    tool_call_meter: SlidingWindowMeter,  // 1min window
    cpu_time_meter: SlidingWindowMeter,   // 24h window
}

/// Snapshot of current quota consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaStatus {
    pub tenant_id: String,
    pub active_sessions: u32,
    pub tokens_consumed: u64,
    pub tool_calls_in_window: usize,
    pub cpu_time_ms_consumed: u64,
}

impl TenantSupervisor {
    pub fn new(tenant: Tenant) -> Self {
        Self {
            tenant,
            active_sessions: AtomicUsize::new(0),
            token_meter: SlidingWindowMeter::new(Duration::from_secs(86400)),
            tool_call_meter: SlidingWindowMeter::new(Duration::from_secs(60)),
            cpu_time_meter: SlidingWindowMeter::new(Duration::from_secs(86400)),
        }
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant.id
    }

    /// Attempt to reserve a session slot. Returns a `SessionGuard` that auto-releases on drop.
    /// Fails if at capacity.
    pub fn reserve_session(self: &Arc<Self>) -> Result<SessionGuard, TenantError> {
        let current = self.active_sessions.fetch_add(1, Ordering::SeqCst) + 1;
        if current > self.tenant.quota.max_concurrent_sessions as usize {
            self.active_sessions.fetch_sub(1, Ordering::SeqCst);
            return Err(TenantError::SessionLimitExceeded {
                tenant_id: self.tenant.id.clone(),
                max: self.tenant.quota.max_concurrent_sessions,
                current: (current - 1) as u32,
            });
        }
        Ok(SessionGuard {
            supervisor: self.clone(),
            released: false,
        })
    }

    /// Release a session slot. Called automatically by `SessionGuard::drop`.
    fn release_session(&self) {
        let prev = self.active_sessions.fetch_sub(1, Ordering::SeqCst);
        if prev == 0 {
            // Underflow protection — reset to 0
            self.active_sessions.store(0, Ordering::SeqCst);
        }
    }

/// RAII guard for a reserved session slot. Auto-releases on drop.
pub struct SessionGuard {
    supervisor: Arc<TenantSupervisor>,
    released: bool,
}

impl SessionGuard {
    /// Explicitly release the session slot before drop.
    pub fn release(mut self) {
        self.supervisor.release_session();
        self.released = true;
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if !self.released {
            self.supervisor.release_session();
        }
    }
}

    /// Record LLM token usage.
    pub fn record_usage(&self, usage: &llm_client::Usage) {
        self.token_meter.record(usage.total_tokens as u64);
    }

    /// Record a tool call.
    pub fn record_tool_call(&self) {
        self.tool_call_meter.record(1);
    }

    /// Record CPU time (wall-clock proxy in ms).
    pub fn record_cpu_time_ms(&self, ms: u64) {
        self.cpu_time_meter.record(ms);
    }

    /// Check whether a quota operation is allowed.
    pub fn check_quota(&self, check: QuotaCheck) -> Result<(), TenantError> {
        match check {
            QuotaCheck::SessionCreation => {
                let current = self.active_sessions.load(Ordering::SeqCst) as u32;
                if current >= self.tenant.quota.max_concurrent_sessions {
                    return Err(TenantError::SessionLimitExceeded {
                        tenant_id: self.tenant.id.clone(),
                        max: self.tenant.quota.max_concurrent_sessions,
                        current,
                    });
                }
            }
            QuotaCheck::ToolCall => {
                let calls = self.tool_call_meter.count();
                if calls >= self.tenant.quota.max_tool_calls_per_minute as usize {
                    return Err(TenantError::ToolCallRateLimitExceeded {
                        tenant_id: self.tenant.id.clone(),
                        calls,
                    });
                }
            }
            QuotaCheck::TokenUsage { input, output } => {
                let total = self.token_meter.sum() + input + output;
                if total > self.tenant.quota.max_tokens_per_day {
                    return Err(TenantError::TokenBudgetExceeded {
                        tenant_id: self.tenant.id.clone(),
                        consumed: self.token_meter.sum(),
                        budget: self.tenant.quota.max_tokens_per_day,
                    });
                }
            }
        }
        Ok(())
    }

    /// Get current quota consumption snapshot.
    pub fn quota_status(&self) -> QuotaStatus {
        QuotaStatus {
            tenant_id: self.tenant.id.clone(),
            active_sessions: self.active_sessions.load(Ordering::SeqCst) as u32,
            tokens_consumed: self.token_meter.sum(),
            tool_calls_in_window: self.tool_call_meter.count(),
            cpu_time_ms_consumed: self.cpu_time_meter.sum(),
        }
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p tenant`
Expected: PASS

---

### Task 6: Implement TenantRegistry

**Files:**
- Create: `crates/tenant/src/registry.rs`

- [ ] **Step 1: Write the failing test**

`crates/tenant/tests/registry.rs`:
```rust
use tenant::{Tenant, TenantQuota, TenantRegistry};
use std::sync::Arc;

#[test]
fn test_registry_register_and_get() {
    let registry = TenantRegistry::new();
    let tenant = Tenant::new("t1", TenantQuota::default());
    registry.register(tenant.clone()).unwrap();

    let sv = registry.get("t1").unwrap();
    assert_eq!(sv.tenant_id(), "t1");
}

#[test]
fn test_registry_duplicate_rejected() {
    let registry = TenantRegistry::new();
    let tenant = Tenant::new("t1", TenantQuota::default());
    registry.register(tenant.clone()).unwrap();

    let err = registry.register(tenant).unwrap_err();
    assert!(matches!(err, tenant::TenantError::TenantAlreadyExists(_)));
}

#[test]
fn test_registry_unknown_tenant() {
    let registry = TenantRegistry::new();
    assert!(registry.get("unknown").is_none());
}

#[tokio::test]
async fn test_registry_concurrent_sessions() {
    let registry = TenantRegistry::new();
    let tenant = Tenant::new("t1", TenantQuota {
        max_concurrent_sessions: 100,
        ..TenantQuota::default()
    });
    registry.register(tenant).unwrap();

    let sv = registry.get("t1").unwrap();

    let handles: Vec<_> = (0..100)
        .map(|_| {
            let sv = sv.clone();
            tokio::spawn(async move {
                sv.reserve_session()
            })
        })
        .collect();

    for h in handles {
        assert!(h.await.unwrap().is_ok());
    }

    assert_eq!(sv.quota_status().active_sessions, 100);
}
```

Run: `cargo test -p tenant`
Expected: FAIL — TenantRegistry not defined

- [ ] **Step 2: Implement TenantRegistry**

`crates/tenant/src/registry.rs`:
```rust
use std::sync::Arc;
use dashmap::DashMap;

use crate::error::TenantError;
use crate::supervisor::TenantSupervisor;
use crate::tenant::Tenant;

/// Global registry of all tenants and their supervisors.
pub struct TenantRegistry {
    tenants: DashMap<String, Arc<TenantSupervisor>>,
}

impl TenantRegistry {
    pub fn new() -> Self {
        Self {
            tenants: DashMap::new(),
        }
    }

    /// Register a new tenant. Fails if tenant_id already exists.
    pub fn register(&self, tenant: Tenant) -> Result<(), TenantError> {
        let id = tenant.id.clone();
        let supervisor = Arc::new(TenantSupervisor::new(tenant));
        match self.tenants.entry(id.clone()) {
            dashmap::mapref::entry::Entry::Occupied(_) => {
                Err(TenantError::TenantAlreadyExists(id))
            }
            dashmap::mapref::entry::Entry::Vacant(e) => {
                e.insert(supervisor);
                Ok(())
            }
        }
    }

    /// Look up a tenant's supervisor.
    pub fn get(&self, tenant_id: &str) -> Option<Arc<TenantSupervisor>> {
        self.tenants.get(tenant_id).map(|entry| entry.clone())
    }

    /// Unregister a tenant, returning its supervisor if it existed.
    pub fn unregister(&self, tenant_id: &str) -> Option<Arc<TenantSupervisor>> {
        self.tenants.remove(tenant_id).map(|(_, v)| v)
    }

    /// Check whether a tenant is registered.
    pub fn contains(&self, tenant_id: &str) -> bool {
        self.tenants.contains_key(tenant_id)
    }

    /// Number of registered tenants.
    pub fn len(&self) -> usize {
        self.tenants.len()
    }
}

impl Default for TenantRegistry {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p tenant`
Expected: PASS

---

### Task 7: Modify agent-core — Add usage to TurnEndCtx

**Files:**
- Modify: `crates/agent-core/src/context.rs`
- Modify: `crates/agent-core/src/loop.rs`

- [ ] **Step 1: Add usage field to TurnEndCtx**

Modify `crates/agent-core/src/context.rs` line 27-33:

```rust
/// Context passed to Extension::on_turn_end
#[derive(Debug, Clone)]
pub struct TurnEndCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub turn_index: u64,
    pub messages: Vec<AgentMessage>,
    pub usage: llm_client::Usage,
}
```

- [ ] **Step 2: Update loop.rs — first on_turn_end (no tool calls)**

Modify `crates/agent-core/src/loop.rs` around line 313-320. Change:
```rust
let turn_ctx = TurnEndCtx {
    tenant_id: self.tenant_id.clone(),
    session_id: self.session_id.clone(),
    turn_index,
    messages: messages.clone(),
    usage: usage.clone(),
};
```

- [ ] **Step 3: Update loop.rs — second on_turn_end (after tool execution)**

Modify `crates/agent-core/src/loop.rs` around line 491-497. Change:
```rust
let turn_ctx = TurnEndCtx {
    tenant_id: self.tenant_id.clone(),
    session_id: self.session_id.clone(),
    turn_index,
    messages: messages.clone(),
    usage: usage.clone(),
};
```

- [ ] **Step 4: Fix tests that construct TurnEndCtx**

Search for any test code constructing `TurnEndCtx` and add the `usage` field.

Run: `cargo test -p agent-core`
Expected: PASS (all existing tests still pass)

---

### Task 8: Implement TenantQuotaExtension

**Files:**
- Create: `crates/tenant/src/extensions/quota.rs`

- [ ] **Step 1: Write the failing test**

`crates/tenant/tests/quota_extension.rs`:
```rust
use std::sync::Arc;
use tenant::{Tenant, TenantQuota, TenantRegistry};
use tenant::extensions::quota::TenantQuotaExtension;
use extensions::Extension;
use agent_core::context::ToolCallCtx;
use agent_core::mutations::HookDecision;

#[tokio::test]
async fn test_quota_extension_allows_within_limit() {
    let registry = Arc::new(TenantRegistry::new());
    let tenant = Tenant::new("t1", TenantQuota {
        max_tool_calls_per_minute: 5,
        ..TenantQuota::default()
    });
    registry.register(tenant).unwrap();

    let ext = TenantQuotaExtension::new(registry);
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    // First 5 calls should pass
    for _ in 0..5 {
        let (decision, _) = ext.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Continue));
    }

    // 6th should be blocked
    let (decision, _) = ext.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
}

#[tokio::test]
async fn test_quota_extension_unknown_tenant() {
    let registry = Arc::new(TenantRegistry::new());
    let ext = TenantQuotaExtension::new(registry);
    let ctx = ToolCallCtx {
        tenant_id: "unknown".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    // Unknown tenant: blocked by default
    let (decision, _) = ext.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
}
```

Run: `cargo test -p tenant`
Expected: FAIL — TenantQuotaExtension not defined

- [ ] **Step 2: Implement TenantQuotaExtension**

`crates/tenant/src/extensions/quota.rs`:
```rust
use std::sync::Arc;
use async_trait::async_trait;
use tracing::warn;

use agent_core::context::ToolCallCtx;
use agent_core::mutations::{HookDecision, ToolCallMutation};
use extensions::host::extension::Extension;

use crate::registry::TenantRegistry;
use crate::tenant::QuotaCheck;

/// Per-tenant quota enforcement extension.
///
/// Checks tool-call rate limits per tenant.
/// Unknown tenants are blocked by default (`allow_unknown = false`).
pub struct TenantQuotaExtension {
    registry: Arc<TenantRegistry>,
    allow_unknown: bool,
}

impl TenantQuotaExtension {
    /// Create extension. `allow_unknown` controls behavior for unregistered tenants.
    /// Default: `false` (block unknown tenants).
    pub fn new(registry: Arc<TenantRegistry>) -> Self {
        Self {
            registry,
            allow_unknown: false,
        }
    }

    /// Opt-in to allow unregistered tenants (e.g., for dev mode).
    pub fn with_allow_unknown(mut self, allow: bool) -> Self {
        self.allow_unknown = allow;
        self
    }
}

#[async_trait]
impl Extension for TenantQuotaExtension {
    fn name(&self) -> &str {
        "tenant-quota"
    }

    async fn on_tool_call(
        &self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        let Some(supervisor) = self.registry.get(&ctx.tenant_id) else {
            if self.allow_unknown {
                warn!(
                    tenant_id = %ctx.tenant_id,
                    "tool call from unregistered tenant — allowing (dev mode)"
                );
                return (HookDecision::Continue, ToolCallMutation::default());
            }
            return (
                HookDecision::Block {
                    reason: format!("tenant '{}' is not registered", ctx.tenant_id),
                },
                ToolCallMutation::default(),
            );
        };

        // Check tool call rate limit
        if let Err(e) = supervisor.check_quota(QuotaCheck::ToolCall) {
            return (
                HookDecision::Block { reason: e.to_string() },
                ToolCallMutation::default(),
            );
        }

        supervisor.record_tool_call();
        (HookDecision::Continue, ToolCallMutation::default())
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p tenant`
Expected: PASS

---

### Task 9: Implement TenantTokenMeterExtension

**Files:**
- Create: `crates/tenant/src/extensions/token_meter.rs`

- [ ] **Step 1: Write the failing test**

`crates/tenant/tests/token_meter.rs`:
```rust
use std::sync::Arc;
use tenant::{Tenant, TenantQuota, TenantRegistry};
use tenant::extensions::token_meter::TenantTokenMeterExtension;
use extensions::Extension;
use agent_core::context::TurnEndCtx;
use llm_client::Usage;

#[tokio::test]
async fn test_token_meter_records_usage() {
    let registry = Arc::new(TenantRegistry::new());
    let tenant = Tenant::new("t1", TenantQuota::default());
    registry.register(tenant).unwrap();

    let ext = TenantTokenMeterExtension::new(registry.clone());
    let ctx = TurnEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        turn_index: 0,
        messages: vec![],
        usage: Usage {
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
    };

    ext.on_turn_end(&ctx).await;

    let sv = registry.get("t1").unwrap();
    let status = sv.quota_status();
    assert_eq!(status.tokens_consumed, 30);
}
```

Run: `cargo test -p tenant`
Expected: FAIL — TenantTokenMeterExtension not defined

- [ ] **Step 2: Implement TenantTokenMeterExtension**

`crates/tenant/src/extensions/token_meter.rs`:
```rust
use std::sync::Arc;
use async_trait::async_trait;
use tracing::warn;

use agent_core::context::TurnEndCtx;
use extensions::host::extension::Extension;

use crate::registry::TenantRegistry;

/// Per-tenant token usage metering extension.
///
/// Records LLM token consumption on each turn end.
pub struct TenantTokenMeterExtension {
    registry: Arc<TenantRegistry>,
}

impl TenantTokenMeterExtension {
    pub fn new(registry: Arc<TenantRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Extension for TenantTokenMeterExtension {
    fn name(&self) -> &str {
        "tenant-token-meter"
    }

    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        let Some(supervisor) = self.registry.get(&ctx.tenant_id) else {
            warn!(
                tenant_id = %ctx.tenant_id,
                "turn_end from unregistered tenant — ignoring"
            );
            return;
        };

        supervisor.record_usage(&ctx.usage);
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p tenant`
Expected: PASS

---

### Task 10: Wire up extension exports

**Files:**
- Create: `crates/tenant/src/extensions/mod.rs`

- [ ] **Step 1: Create mod.rs**

`crates/tenant/src/extensions/mod.rs`:
```rust
pub mod quota;
pub mod token_meter;
```

- [ ] **Step 2: Verify full crate compiles**

Run: `cargo check -p tenant`
Expected: succeeds

---

### Task 11: Integration test — end-to-end multi-tenant isolation

**Files:**
- Create: `crates/tenant/tests/integration.rs`

- [ ] **Step 1: Write integration test**

`crates/tenant/tests/integration.rs`:
```rust
use std::sync::Arc;
use tenant::{Tenant, TenantQuota, TenantRegistry};
use tenant::extensions::quota::TenantQuotaExtension;
use tenant::extensions::token_meter::TenantTokenMeterExtension;
use extensions::{ExtensionManager, Extension};
use agent_core::{AgentLoop, HookDispatcher, SessionActor};
use agent_core::types::AgentMessage;
use llm_client::{Content, LlmContext, LlmProvider, StreamOptions, StopReason, AssistantMessage, Usage};
use tokio_util::sync::CancellationToken;

// Echo provider for testing
struct EchoProvider;
#[async_trait::async_trait]
impl LlmProvider for EchoProvider {
    fn provider_name(&self) -> &str { "echo" }
    fn models(&self) -> Vec<String> { vec!["echo".to_string()] }
    async fn stream(
        &self,
        _model: &str,
        _ctx: LlmContext,
        _opts: StreamOptions,
        _signal: CancellationToken,
    ) -> Result<llm_client::AssistantMessageEventStream, llm_client::LlmError> {
        let (stream, tx) = llm_client::AssistantMessageEventStream::new(4);
        let msg = AssistantMessage {
            content: vec![Content::Text { text: "hi".to_string(), text_signature: None }],
            provider: "echo".to_string(),
            model: "echo".to_string(),
            api: llm_client::Api { provider: "echo".to_string(), model: "echo".to_string() },
            usage: Usage { input_tokens: 5, output_tokens: 5, total_tokens: 10, cache_creation_input_tokens: None, cache_read_input_tokens: None },
            stop_reason: StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        };
        tokio::spawn(async move {
            let _ = tx.send(llm_client::AssistantMessageEvent::Start { partial: msg.clone() }).await;
            let _ = tx.send(llm_client::AssistantMessageEvent::Done { reason: StopReason::Stop, message: msg }).await;
        });
        Ok(stream)
    }
}

#[tokio::test]
async fn test_end_to_end_tenant_isolation() {
    let _ = tracing_subscriber::fmt().try_init();

    // Set up registry with two tenants
    let registry = Arc::new(TenantRegistry::new());

    let t1 = Tenant::new("t1", TenantQuota {
        max_concurrent_sessions: 2,
        max_tokens_per_day: 100,
        max_tool_calls_per_minute: 10,
        ..TenantQuota::default()
    });
    let t2 = Tenant::new("t2", TenantQuota {
        max_concurrent_sessions: 5,
        max_tokens_per_day: 200,
        max_tool_calls_per_minute: 20,
        ..TenantQuota::default()
    });

    registry.register(t1).unwrap();
    registry.register(t2).unwrap();

    // Create extensions
    let quota_ext = Arc::new(TenantQuotaExtension::new(registry.clone()));
    let meter_ext = Arc::new(TenantTokenMeterExtension::new(registry.clone()));

    let manager = ExtensionManager::new(vec![quota_ext, meter_ext]);
    let (hook_dispatcher, handles, _joins) = manager.spawn_all();
    let dispatcher: Arc<dyn HookDispatcher> = Arc::new(hook_dispatcher);

    let provider: Arc<dyn LlmProvider> = Arc::new(EchoProvider);

    // Tenant 1: reserve + create session (SessionGuard auto-releases)
    let t1_sv = registry.get("t1").unwrap();
    let _t1_guard = t1_sv.reserve_session().unwrap();

    let mut session1 = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".to_string(),
        "echo".to_string(),
        128_000,
        provider.clone(),
        dispatcher.clone(),
        vec![],
        None,
        None,
    );

    session1.prompt("hello".to_string()).await.unwrap();

    // Verify t1 token usage was recorded
    let t1_status = t1_sv.quota_status();
    assert_eq!(t1_status.tokens_consumed, 10);

    // Tenant 2: reserve + create session
    let t2_sv = registry.get("t2").unwrap();
    let _t2_guard = t2_sv.reserve_session().unwrap();

    let mut session2 = SessionActor::new(
        "t2".to_string(),
        "s2".to_string(),
        "You are helpful.".to_string(),
        "echo".to_string(),
        128_000,
        provider.clone(),
        dispatcher.clone(),
        vec![],
        None,
        None,
    );

    session2.prompt("world".to_string()).await.unwrap();

    // Verify t2 token usage was recorded independently
    let t2_status = t2_sv.quota_status();
    assert_eq!(t2_status.tokens_consumed, 10);

    // t1 should still only have 10 (not affected by t2)
    let t1_status = t1_sv.quota_status();
    assert_eq!(t1_status.tokens_consumed, 10);

    // SessionGuards auto-release on drop — no manual cleanup needed
    for h in handles {
        h.shutdown().await;
    }
}
```

- [ ] **Step 2: Run integration test**

Run: `cargo test -p tenant --test integration`
Expected: PASS

---

### Task 12: Deprecate global RateLimitExtension

**Files:**
- Modify: `crates/extensions/src/builtins/rate_limit.rs`

- [ ] **Step 1: Add deprecation doc comment**

Add to `crates/extensions/src/builtins/rate_limit.rs` line 11:
```rust
/// Rate-limit extension — sliding-window tool call frequency limit.
///
/// ⚠️ DEPRECATED: This is a global rate limiter shared across all tenants.
/// Use `tenant::extensions::quota::TenantQuotaExtension` for per-tenant enforcement.
```

- [ ] **Step 2: Verify no breakage**

Run: `cargo check -p extensions`
Expected: succeeds (doc comment only)

---

### Task 13: Documentation

**Files:**
- Create: `crates/tenant/README.md`

- [ ] **Step 1: Write README.md**

```markdown
# tenant

Per-tenant registry, quota management, session tracking, and resource metering.

## Responsibility

This crate is the multi-tenancy control plane. It sits between `api-gateway`
(and other entry points) and `agent-core`, enforcing per-tenant resource
boundaries before sessions are created.

## Public API

- `TenantRegistry` — global concurrent registry of all tenants.
- `TenantSupervisor` — per-tenant session tracker and quota enforcer.
- `TenantQuota` — configurable limits (sessions, tokens, tool calls, CPU).
- `TenantQuotaExtension` — per-tenant tool-call rate limit (blocking hook).
- `TenantTokenMeterExtension` — per-tenant token usage metering (observational hook).
- `SessionGuard` — RAII guard for session slots, auto-releases on drop.

## Usage Flow

1. **Registration**: At startup (or on first request), register tenants:
   ```rust
   let registry = Arc::new(TenantRegistry::new());
   registry.register(Tenant::new("acme", TenantQuota::default()))?;
   ```

2. **Session creation**: Before creating a `SessionActor`, reserve a slot:
   ```rust
   let sv = registry.get("acme").ok_or(...)?;
   sv.check_quota(QuotaCheck::SessionCreation)?;
   let _guard = sv.reserve_session()?; // auto-releases on drop
   let session = SessionActor::new("acme", ..., hook_dispatcher, ...);
   ```

3. **Inline enforcement**: Register extensions in order:
   ```rust
   let manager = ExtensionManager::new(vec![
       Arc::new(TenantQuotaExtension::new(registry.clone())),
       Arc::new(TenantTokenMeterExtension::new(registry.clone())),
   ]);
   ```
   - `TenantQuotaExtension` must come **before** `TenantTokenMeterExtension`.
   - Unknown tenants are **blocked by default** (`allow_unknown: false`).

4. **Token budget enforcement**: Call `check_quota(TokenUsage)` at the
   api-gateway or session-factory layer before accepting new prompts.
   Inline token-budget blocking inside the Extension system is not yet
   supported (requires `on_before_provider_request` hook, marked TODO).

## Boundaries

- **Does not** create `SessionActor` instances — that's the caller's responsibility.
- **Does not** handle authentication/authorization — assumes `tenant_id` is
  already validated by `api-gateway`.
- **Does not** persist quota counters across restarts (MVP: in-memory sliding windows).
- **Does not** enforce CPU time budget — `cpu_time_budget_ms_per_day` is reserved
  for future use (measurement and enforcement not yet implemented).
```

- [ ] **Step 2: Verify workspace compiles**

Run: `cargo check --workspace`
Expected: succeeds

---

### Task 14: Final verification

- [ ] **Step 1: Run all tenant tests**

Run: `cargo test -p tenant`
Expected: all tests PASS

- [ ] **Step 2: Run all workspace tests**

Run: `cargo test --workspace`
Expected: all tests PASS

- [ ] **Step 3: Check for clippy warnings**

Run: `cargo clippy -p tenant -- -D warnings`
Expected: no warnings (or fix them)

---

## Rollout / Next Steps After This Plan

1. **api-gateway integration**: Wire `TenantRegistry` into the server-side request
   handler. Extract `tenant_id` from JWT/Bearer token, validate against registry,
   call `reserve_session()` before `SessionActor::new()`.

2. **Configuration loading**: Load tenant list and quotas from config file or
   database at startup instead of hard-coding.

3. **Distributed quota**: Replace in-memory `SlidingWindowMeter` with Redis-backed
   counters so quotas are shared across horizontally-scaled instances.

4. **Observability**: Export per-tenant metrics (active sessions, token burn rate,
   quota exhaustion events) to Prometheus.
