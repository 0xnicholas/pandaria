# E2E Coverage Gap — Implementation Plan

> **Status:** Completed ✅ — all 4 test suites delivered and passing

**Goal:** Add 4 missing E2E test suites to Pandaria, prioritized by security and production criticality.

**Architecture:** Each suite lives in `crates/api-gateway/tests/e2e/` as a standalone file, using the existing `common.rs` fixture pattern (wiremock LLM + `TenantManagerImpl` + `build_router`). No crate API changes — only tests.

**Tech Stack:** Rust, tokio, axum TestClient, wiremock, `tower::ServiceExt::oneshot`

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/api-gateway/tests/e2e/e2e_path_guard.rs` | Verify PathGuard blocks file access outside tenant workspace |
| `crates/api-gateway/tests/e2e/e2e_media_provider.rs` | Verify MediaGenerationTool with mock MediaProvider (base64 inline + file save) |
| `crates/api-gateway/tests/e2e/e2e_rate_limit.rs` | Verify TokenBucket rate limiting returns 429 after burst exhaustion |
| `crates/api-gateway/tests/e2e/e2e_token_budget.rs` | Verify max_turns_per_session logs warning / blocks after limit |

---

## Task 1: PathGuard E2E

**Context:**
- `DefaultHookDispatcher` in `agent-core/src/hook/default_dispatcher.rs` handles `on_tool_call`.
- PathGuard checks `path_guard_fields` (tool_name → field names) and blocks paths outside `AgentSpace::workspace_for(tenant_id)`.
- `path_guard_scan_unknown` enables scanning all unknown tools for path-like strings.
- To test at api-gateway level, we need a tool that reads/writes files. The `read_file` tool is registered by default in `SessionActor`.
- We need to configure `HookConfig` with `path_guard_fields` and `path_guard_scan_unknown = true`.

**Files:**
- Create: `crates/api-gateway/tests/e2e/e2e_path_guard.rs`
- Reference: `crates/api-gateway/tests/e2e/common.rs`, `crates/api-gateway/tests/e2e/e2e_tool_use_http.rs`

- [ ] **Step 1: Write failing test — path_guard blocks access outside workspace**

```rust
#[tokio::test]
async fn test_path_guard_blocks_file_outside_workspace() {
    // Build app with path_guard_fields = {"read_file": ["path"]}
    // Send message that triggers tool call with path = "/etc/passwd"
    // LLM mock responds with ToolCall for read_file targeting /etc/passwd
    // Expect: tool result is error / blocked, or SSE contains error event
}
```

- [ ] **Step 2: Write failing test — path_guard allows access inside workspace**

```rust
#[tokio::test]
async fn test_path_guard_allows_file_inside_workspace() {
    // Create a temp file inside workspace
    // Send message triggering read_file for that path
    // Expect: success
}
```

- [ ] **Step 3: Write failing test — path_guard with scan_unknown**

```rust
#[tokio::test]
async fn test_path_guard_scan_unknown_blocks_illegal_paths() {
    // Custom tool with path argument not in path_guard_fields
    // scan_unknown = true → should still block
}
```

- [ ] **Step 4: Run tests, fix any wiring issues**

Run: `cargo test -p api-gateway --test e2e_path_guard -- --nocapture`

- [ ] **Step 5: Commit**

---

## Task 2: MediaProvider E2E

**Context:**
- `MediaGenerationTool` in `agent-core/src/tools/media_generation.rs` implements `AgentTool`.
- It requires a `MediaProvider` + `MediaModelRegistry` in `HarnessConfig`.
- We need a mock `MediaProvider` that returns `MediaResponse::Inline` (base64 image).
- The tool returns inline image if < 1MB, or saves to workspace otherwise.
- To trigger tool use via api-gateway, the LLM mock must respond with a `ToolCall` for `generate_media`.

**Files:**
- Create: `crates/api-gateway/tests/e2e/e2e_media_provider.rs`
- Modify: `crates/api-gateway/tests/e2e/common.rs` — add helper `build_test_app_with_media()`

- [ ] **Step 1: Create mock MediaProvider**

```rust
struct MockMediaProvider;
#[async_trait]
impl MediaProvider for MockMediaProvider {
    fn provider_name(&self) -> &str { "mock" }
    fn supported_tasks(&self) -> Vec<MediaTaskType> { vec![MediaTaskType::ImageGeneration] }
    async fn generate(&self, _model: &str, _req: MediaRequest, _signal: CancellationToken) -> Result<MediaResponse, MediaError> {
        Ok(MediaResponse::Inline { data: "iVBORw0KGgo...".to_string(), mime_type: "image/png".to_string() })
    }
    fn client(&self) -> &reqwest::Client { /* static client */ }
}
```

- [ ] **Step 2: Create MediaModelRegistry with mock model**

```rust
let mut registry = MediaModelRegistry::new();
registry.register(MediaModel { id: "mock-img".into(), provider: "mock".into(), supported_tasks: vec![MediaTaskType::ImageGeneration], cost_per_call: Some(0.01) });
```

- [ ] **Step 3: Build test app with media provider injected**

Modify `common.rs` or inline a builder that sets `media_provider` and `media_registry` in `HarnessConfig`.

- [ ] **Step 4: Write test — generate_media returns inline image**

Wiremock LLM responds with `ToolCall` for `generate_media` with `{"media_type":"image","prompt":"a cat"}`.
Tool result should contain `Content::Image` with base64 data.

- [ ] **Step 5: Write test — generate_media saves large image to workspace**

Mock returns large base64 (>1MB raw after decode). Tool should save to `AgentSpace::media_dir(tenant_id)` and return file path.

- [ ] **Step 6: Run tests**

Run: `cargo test -p api-gateway --test e2e_media_provider -- --nocapture`

- [ ] **Step 7: Commit**

---

## Task 3: Rate Limit E2E

**Context:**
- `RateLimiter` in `crates/api-gateway/src/middleware/rate_limit.rs` uses TokenBucket per tenant.
- `rate_limit_middleware` is applied to all `/api/v1/*` routes.
- Config: `RateLimitConfig { requests_per_second, burst_size }`.
- When exceeded, returns `GatewayError::RateLimited { retry_after }` → HTTP 429.

**Files:**
- Create: `crates/api-gateway/tests/e2e/e2e_rate_limit.rs`
- Reference: `crates/api-gateway/tests/e2e/e2e_session_lifecycle.rs`

- [ ] **Step 1: Write failing test — burst allowed then 429**

```rust
#[tokio::test]
async fn test_rate_limit_blocks_after_burst() {
    // Build app with RateLimitConfig { rps: 100, burst_size: 2 }
    // Send 3 rapid-fire requests to /api/v1/sessions
    // Expect: first 2 → 201, third → 429
}
```

- [ ] **Step 2: Write failing test — per-tenant isolation**

```rust
#[tokio::test]
async fn test_rate_limit_per_tenant_isolation() {
    // Tenant A exhausts burst
    // Tenant B should still get 201
}
```

- [ ] **Step 3: Write failing test — refill after delay**

```rust
#[tokio::test]
async fn test_rate_limit_refills_after_delay() {
    // Exhaust burst, wait > 1/rps, request should succeed again
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p api-gateway --test e2e_rate_limit -- --nocapture`

- [ ] **Step 5: Commit**

---

## Task 4: Token Budget E2E

**Context:**
- `DefaultHookDispatcher::on_before_provider_request` checks `max_turns_per_session`.
- `on_turn_end` increments the counter in `DashMap<String, AtomicUsize>`.
- When exceeded, it logs a warning but does NOT block (non-blocking by design).
- To make it block, we'd need to modify behavior — but the plan says "no crate API changes". So the test verifies:
  1. Warning log is emitted (via `tracing` capture or observation)
  2. OR we test that the counter increments correctly by observing side effects

Actually, looking at the code more carefully:
```rust
if count >= self.max_turns_per_session {
    tracing::warn!(... "budget_exceeded");
}
```
It only warns, doesn't block. The spec says "TokenBudget: per-session turn counting (non-blocking, logs warning)".

So the E2E test should verify:
- After N turns, the warning is logged.
- To capture logs, we can use `tracing-test` or check that the session continues to work.

Alternative: we can test that the `DefaultHookDispatcher` blocks in `on_tool_call` when a tool is denied (ToolGuard), which is related to the same dispatcher. But TokenBudget specifically is non-blocking.

Better approach: verify the behavior at the api-gateway level by sending N+1 messages and confirming the session still processes them (non-blocking), while checking that tracing warns.

For capturing tracing in tests:
```rust
use tracing_subscriber::layer::SubscriberExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

let warned = Arc::new(AtomicBool::new(false));
let w = warned.clone();
let subscriber = tracing_subscriber::registry()
    .with(tracing_subscriber::fmt::layer().with_test_writer())
    .with(tracing_subscriber::Layer::new()); // custom layer to detect warning
```

Actually a simpler approach: since the counter is in `DefaultHookDispatcher` which is inside `SessionActor`, we don't have direct access. But we can verify that after max_turns, the session still accepts messages (proving non-blocking).

Let's simplify:
1. `test_token_budget_non_blocking` — send 5 messages with max_turns=2, all succeed
2. `test_token_budget_logs_warning` — use a custom tracing layer to capture the warning

- [ ] **Step 1: Write test — token budget is non-blocking**

```rust
#[tokio::test]
async fn test_token_budget_does_not_block() {
    // Build app with max_turns_per_session = 2
    // Send 4 messages
    // All should return 200 OK
}
```

- [ ] **Step 2: Write test — token budget logs warning**

Use `tracing_subscriber::fmt` with a custom layer or just verify session continues.

- [ ] **Step 3: Run tests**

Run: `cargo test -p api-gateway --test e2e_token_budget -- --nocapture`

- [ ] **Step 4: Commit**

---

## Execution Order

1. Task 1 (PathGuard) — security critical, no mock dependencies beyond LLM
2. Task 2 (MediaProvider) — requires mock MediaProvider
3. Task 3 (Rate Limit) — pure api-gateway, no agent-core involvement
4. Task 4 (Token Budget) — lightweight, uses existing tracing infra

---

## Verification Commands

```bash
# After all tasks:
cargo test -p api-gateway --test e2e_path_guard
cargo test -p api-gateway --test e2e_media_provider
cargo test -p api-gateway --test e2e_rate_limit
cargo test -p api-gateway --test e2e_token_budget
```
