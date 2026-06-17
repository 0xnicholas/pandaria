## Review: PandariaAgentExecutor Design

**status:** `issues_found`

**summary:** The design correctly identifies the goal (wire `SquadEngine` to `agent-core::SessionActor`) and the right files/exports, but several concrete API, concurrency, and integration details are inconsistent with the existing `AgentExecutor` trait, `SessionBuilder` API, and the broader Tavern Agent Team design. It should not be approved for implementation without resolving the required issues below.

---

## Correct

- **File placement and exports are consistent.** Adding `crates/tavern-comp/src/team/pandaria_executor.rs` and re-exporting through `team/mod.rs` and `lib.rs` matches the existing module structure (`team/engine.rs`, `team/executor.rs`, etc.).
- **Use of `SessionBuilder`/`SessionActor` is the right integration point.** `HarnessConfig` already aggregates the provider, store, compaction, hooks, and `AgentSpace`, so passing it through is aligned with `agent-core` usage (see `agent-core/src/harness/builder.rs`, `SessionBuilder::build`).
- **`AgentOutput.content` as `Value::String(text)` is correct** given that `SessionActor::complete()` returns `String` and `SquadEngine` already runs `Handoff::detect(&output.content)` itself (`team/engine.rs:350`).
- **Adding `squad_id` to `AgentInput` for tracing is sensible**, provided the trait/struct is updated consistently.

---

## Issues

### 1. `acquire_session` concurrency fallback is internally inconsistent and racy
**severity:** required

**description:** The doc first claims "并发调用冲突时，回退到临时 SessionActor", but the initial `acquire_session` pseudocode just returns an `Arc<Mutex<SessionActor>>` and lets callers serialize on `lock().await`. It then adds a "修正" proposing `try_lock_owned()`. However:
- `tokio::sync::Mutex::try_lock_owned(self: Arc<Self>)` returns an `OwnedMutexGuard<T>`, not an `Arc<Mutex<T>>`. The proposed return type `(Arc<Mutex<SessionActor>>, bool)` cannot represent a held reused session.
- The initial code has a check-then-build race: two tasks can both see an empty map, both build a session, and one overwrites the other in `map.entry(...).or_insert_with(...)`.
- It is unclear who owns the lock for a temporary session, when it is released, and how `flush()` accounts for it.

**suggested fix:** Redesign `acquire_session` to return an enum such as:
```rust
enum SessionHandle {
    Reused(OwnedMutexGuard<SessionActor>),
    Temp(tokio::sync::MutexGuard<SessionActor>),
}
```
Or keep the `Arc<Mutex<...>>` contract but make the caller attempt `try_lock()` under the map lock and create a temp session only on `TryLockError::WouldBlock`. Document that temp sessions are never cached and never flushed by `PandariaAgentExecutor::flush`.

---

### 2. Tool type mismatch between Tavern skills and `SessionBuilder`
**severity:** required

**description:** The doc proposes calling `crate::hero::skills_to_tool_defs(&agent.skills)` and passing the result to `SessionBuilder::with_external_tools(...)`. `skills_to_tool_defs` returns `Vec<crate::agent::ToolDef>` (a Tavern type), while `with_external_tools` expects `Vec<agent_core::tools::ToolConfig>` (`agent-core/src/harness/builder.rs`). These are different structs.

**suggested fix:** Convert `tavern_comp::agent::ToolDef` to `agent_core::tools::ToolConfig` (fields map directly: name, description, parameters, endpoint, timeout_ms, headers), or add a helper in `tavern-comp/src/agent.rs` that produces `Vec<agent_core::tools::ToolConfig>` and use that instead of the Hero HTTP-endpoint helper.

---

### 3. Duplicate skill injection when reusing `build_system_prompt`
**severity:** required

**description:** The doc wants to reuse `TavernHero::build_system_prompt`, which appends an `## Available Skills` block to the prompt string. However, `SessionBuilder::build()` already injects skills via `PromptBuilder::upsert_fragment("skills-directory", ...)` when `config.skills` is non-empty (`agent-core/src/harness/builder.rs`). Passing a skill-injected system prompt into `SessionBuilder` will cause skill descriptions to appear twice.

**suggested fix:** Either (a) move/reuse only the constraints/instructions logic and let `SessionBuilder` handle skills, or (b) build the system prompt without the skills block and pass it to `SessionBuilder`. Prefer (a) to keep skill injection centralized in `agent-core`.

---

### 4. Session cache key ignores model, so `model_override` is silently dropped on reuse
**severity:** required

**description:** `acquire_session` uses only `role_id` as the cache key. `AgentInput` carries `model_override`, and `execute` formats a `model` string, but once a session is created for a role it will keep using the original model. `SessionActor::set_model` exists but the doc never calls it.

**suggested fix:** Include the resolved model string in the cache key (e.g. `format!("{}:{}", role_id, model)`), or compare the requested model against `actor.model()` and call `set_model()` when they differ. The latter is cheaper but mutates shared session state; the former is safer for isolation.

---

### 5. `resolve_role` cannot produce the team-level `Role` fields required by the broader design
**severity:** required

**description:** The broader Tavern design (`docs/superpowers/specs/2026-06-16-tavern-agent-team-design.md`, §3.2) defines `Role` with `team_instructions` and `visibility` that live on the `Team`. The `AgentExecutor::resolve_role` trait only receives `role_id`, so `PandariaAgentExecutor` can only build a `Role` from `AgentConfig`, losing `team_instructions` and `visibility`. `SquadEngine` currently calls `squad.executor.resolve_role(&mission.role)` and then only uses `role.model_override`, so team-level fields are effectively ignored end-to-end.

**suggested fix:** Change the trait so `SquadEngine` resolves the `Role` from `team.roles` directly and passes it (or just `agent_id`) to the executor, while the executor exposes a method to resolve the underlying `AgentConfig`. At minimum, document that `resolve_role` returns only the agent-derived subset and that team-level fields must be merged by `SquadEngine`.

---

### 6. Timeout on `actor.complete()` leaves `SessionActor` in an inconsistent state
**severity:** required

**description:** The doc wraps `actor.complete(prompt).await` in `tokio::time::timeout`. `SessionActor` sets `state = Running` during `run_with_messages` and only resets it on normal completion (`agent-core/src/harness/session.rs`). If the timeout fires, the future is dropped, `state` remains `Running`, message entries may be partially mutated, and the in-flight `last_save` handle may be in an undefined state. The next caller that reuses this session will see stale state.

**suggested fix:** Do not timeout inside `execute`; rely on `AgentInput.timeout` being passed to `SessionActor` once that method supports a deadline/cancellation token, or call `actor.abort_token().cancel()` on timeout and then discard the session (remove it from the cache) rather than returning it for reuse. Document that timeout currently poisons the cached session.

---

### 7. `AgentOutput.usage` cannot be populated, contradicting team-level aggregation requirements
**severity:** required

**description:** The broader design (§7.1, §6.1) says `PandariaAgentExecutor` should return `Usage` for Tavern to aggregate team token consumption. The doc returns `usage: None` because `SessionActor::complete()` returns only `String`. There is no public API on `SessionActor` to retrieve the last turn's usage.

**suggested fix:** Either add a method to `SessionActor` to expose last-turn usage (e.g. `last_usage() -> Option<Usage>`) and consume it in `execute`, or update the broader design and `AgentOutput` to make usage optional at P0 with a TODO to add metering in a follow-up.

---

### 8. `flush()` is not reachable through the trait boundary used by `SquadEngine`
**severity:** required

**description:** The doc adds `flush()` as a concrete method on `PandariaAgentExecutor` and explicitly avoids adding it to `AgentExecutor`. But `Squad` stores `Arc<dyn AgentExecutor>`, and `SquadEngine` never downcasts. Callers that only hold `Arc<dyn AgentExecutor>` cannot flush, so SquadEngine cannot guarantee persistence before returning a `SquadResult`.

**suggested fix:** Add `flush(&self) -> Result<(), AgentExecutorError>` to the `AgentExecutor` trait with a default no-op implementation so mock executors are unaffected, and call it from `SquadEngine::run` after completion/failure. Alternatively, document that callers must downcast to `PandariaAgentExecutor` and flush manually, but that breaks the abstraction.

---

### 9. `skills_to_tool_defs` depends on Tavern HTTP env vars and is inappropriate for PandariaExecutor
**severity:** required

**description:** `TavernHero::skills_to_tool_defs` returns an empty vector unless both `TAVERN_PUBLIC_URL` and `TAVERN_TOOL_SECRET` are set (`crates/tavern-comp/src/hero/hero.rs`). A production `PandariaAgentExecutor` that goes through `agent-core` directly should not require Tavern HTTP endpoint configuration to register skills as tools.

**suggested fix:** Build agent-core `ToolConfig`s directly from `AgentConfig.skills` in `PandariaAgentExecutor`, bypassing `skills_to_tool_defs`. Use `agent-core::tools::ToolConfig` and `HttpProxyTool` only if the skill runner is `Sidecar`; for `Rust`/subprocess skills, register in-process tools or document the limitation.

---

### 10. Tracing span is missing required `team_id` and `mission_id`
**severity:** required

**description:** The broader design's observability section (§7.2) requires every span to carry `tenant_id`, `squad_id`, `team_id`, `role_id`, and `mission_id`. The proposed `#[tracing::instrument(...)]` only includes `tenant_id`, `role_id`, and `squad_id`.

**suggested fix:** Add `team_id` to `PandariaAgentExecutor` construction and include it in the span. Add `mission_id` to `AgentInput` (or derive it from context) and pass it through; update `SquadEngine::execute_mission` to populate it.

---

### 11. Lossy error mapping hides error taxonomy
**severity:** recommended

**description:** The doc maps all `AgentError` and `SessionBuilder::build` failures to `AgentExecutorError::ExecutionFailed(String)`. This makes it impossible for `SquadEngine` or callers to distinguish provider errors, context-overflow, tool-denial, quota exhaustion, etc.

**suggested fix:** Add more `AgentExecutorError` variants (e.g. `Provider`, `ContextOverflow`, `ToolDenied`) and map from `AgentError` explicitly, or at minimum include the original `AgentError` in a `source` field if `thiserror` `#[source]` is used.

---

### 12. Temporary sessions are not flushed, risking lost persistence writes
**severity:** recommended

**description:** The doc states that temporary fallback sessions are discarded after execution. `SessionActor` persists incrementally via fire-and-forget `tokio::spawn`; if the actor is dropped before `flush()` completes, the last in-flight `last_save` join handle is dropped without awaiting. Under graceful shutdown this can lose the final entries.

**suggested fix:** Track temporary sessions in a separate `Arc<Mutex<Vec<Arc<Mutex<SessionActor>>>>>` and include them in `flush()`, or call `actor.flush().await` before dropping a temporary session in `execute`.

---

### 13. `std::sync::Mutex` lock uses `.unwrap()` instead of `.expect()`
**severity:** recommended

**description:** The doc uses `self.sessions.lock().unwrap()`. The project convention (AGENTS.md, §代码规范) is to avoid production `.unwrap()` and use `.expect("reason")`.

**suggested fix:** Replace with `.expect("pandaria executor session map poisoned")`, or use `parking_lot::Mutex` which does not poison.

---

### 14. `PandariaAgentExecutor` is tightly coupled to `TavernHero`
**severity:** recommended

**description:** The design injects `Arc<TavernHero>`, which includes an `AgentRuntime` that `PandariaAgentExecutor` does not use. The broader design says `TavernHero` will migrate to `AgentExecutor + RoleRegistry`.

**suggested fix:** Define a small trait such as `AgentResolver: async fn resolve(&self, agent_id: &str) -> Option<AgentConfig>` and implement it for `TavernHero`. Inject `Arc<dyn AgentResolver>` into `PandariaAgentExecutor`. This makes testing easier and aligns with the migration.

---

### 15. No bound on temporary fallback sessions
**severity:** recommended

**description:** If many missions for the same role run concurrently, each gets its own full `SessionActor` with tools, compactor, and provider. There is no semaphore or limit on fallback sessions, which can exhaust memory or provider quota.

**suggested fix:** Add a configurable max concurrency limit (e.g. an `Arc<Semaphore>`) and either queue or fail when the limit is exceeded.

---

## Notes

- The `execute_stream` stub returning `futures_util::stream::empty()` is acceptable for P0, but the trait return type forces a `'static` stream, which is fine.
- The prompt-building function `build_role_prompt` is only sketched. This is OK for a design doc, but its implementability depends on a clear contract for serializing `TeamContext` into a single user message; consider limiting thread length and context size to avoid unexpectedly large prompts.
- `PandariaAgentExecutor` claims to inherit "并发/配额控制". `SessionActor` provides session isolation but not tenant-level CPU-time/concurrent-session enforcement; that belongs to the `tenant` crate or the api-gateway layer. The doc should clarify what is inherited versus what still requires upstream enforcement.
- Consider whether `flush()` should also be invoked automatically when the last `PandariaAgentExecutor` clone is dropped (`Drop` on the inner struct) to reduce the risk of callers forgetting to call it.
