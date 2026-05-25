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
- `TenantManager` trait — dependency-inversion boundary consumed by `api-gateway`.
- `TenantManagerImpl` — default implementation managing `SessionActor` lifecycle.
- `TenantQuotaExtension` — per-tenant tool-call rate limit (blocking hook, via `DefaultHookDispatcher`).
- `TenantTokenMeterExtension` — per-tenant token usage metering (observational hook, via `DefaultHookDispatcher`).
- `SessionGuard` — RAII guard for session slots, auto-releases on drop.

## Usage Flow

1. **Registration**: At startup (or on first request), register tenants:
   ```rust
   let registry = Arc::new(TenantRegistry::new());
   registry.register(Tenant::new("acme", TenantQuota::default()))?;
   ```

2. **Manager creation**: Construct `TenantManagerImpl` with all dependencies:
   ```rust
   let manager = TenantManagerImpl::new(
       registry,
       provider,           // Arc<dyn LlmProvider>
       store,              // Option<Arc<dyn SessionStore>>
       "claude-sonnet-4",  // default model
       "You are helpful.", // default system prompt
       128_000,            // default context window
       hook_dispatcher,    // Arc<dyn HookDispatcher>
   );
   ```

3. **Session creation**: `TenantManagerImpl::create_session()` automatically:
   - Validates tenant exists
   - Checks session quota
   - Reserves a session slot (`SessionGuard`)
   - Creates `CompactionActor` + `SessionActor` with the provided `HookDispatcher`
   - Sets up event bridge for SSE subscriptions

4. **Inline enforcement**: Configure `DefaultHookDispatcher` with tenant extensions:
   ```rust
   use agent_core::hook::DefaultHookDispatcher;
   use tenant::{TenantQuotaExtension, TenantTokenMeterExtension};

   let hook_dispatcher = DefaultHookDispatcher::builder()
       .space(space.clone())
       .custom_on_tool_call(Box::new(TenantQuotaExtension::new(registry.clone())))
       .custom_on_turn_end(Box::new(TenantTokenMeterExtension::new(registry.clone())))
       .build();

   let manager = TenantManagerImpl::new(
       registry.clone(),
       provider,
       store,
       model,
       system_prompt,
       context_window,
       Arc::new(hook_dispatcher),
   );
   ```
   - `TenantQuotaExtension` should be registered as a blocking hook (e.g., `on_tool_call`).
   - `TenantTokenMeterExtension` should be registered as an observational hook (e.g., `on_turn_end`).
   - Unknown tenants are **blocked by default** (`allow_unknown: false`).

5. **Token budget enforcement**: Call `check_quota(TokenUsage)` at the
   api-gateway or session-factory layer before accepting new prompts.
   Inline token-budget blocking inside the hook system requires
   `on_before_provider_request` hook support (available via `DefaultHookDispatcher`).

## Boundaries

- **Does not** create `SessionActor` instances directly — that's `TenantManagerImpl`'s internal responsibility.
- **Does not** handle authentication/authorization — assumes `tenant_id` is
  already validated by `api-gateway`.
- **Does not** persist quota counters across restarts (MVP: in-memory sliding windows).
- **Does not** enforce CPU time budget — `cpu_time_budget_ms_per_day` is reserved
  for future use (measurement and enforcement not yet implemented).
- **Note**: `interrupt()` cancels the `CancellationToken` but does not reset it;
  subsequent `send_message()` calls on the same session rely on `SessionActor`
  to recreate or re-arm the token as needed.
