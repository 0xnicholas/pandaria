---

> **⚠️ DEPRECATED — Architecture Changed (v0.1.x)**
>
> This document was written for the **Extension Actor + EventBus** architecture, which has been **removed** in v0.1.x.
> The "extensions" crate, ExtensionActor, HookRouter, and EventBus no longer exist.
> Built-in strategies (audit, path_guard, tool_guard, token_budget) are now inlined in agent-core::hook::DefaultHookDispatcher.
> Hook calls are direct function calls (no Actor, no EventBus, no timeout boundaries).
> See [AGENTS.md](../../AGENTS.md) (ADR-002, ADR-003) for the current architecture.

---

# PromptBuilder Implementation Plan

> Status: Approved  
> Created: 2026-05-15  
> Spec: `docs/specs/prompt-builder.md`

---

## 1. 里程碑

### M1: PromptBuilder Core Types
**Scope**: Create `agent-core/src/prompt/` module with builder, fragment, mutation, and tests.

**Files**:
- `agent-core/src/prompt/mod.rs` — module exports
- `agent-core/src/prompt/types.rs` — `PromptFragment`, `FragmentKind`, `FragmentSource`, `RenderedPrompt`, `RenderedFragment`
- `agent-core/src/prompt/builder.rs` — `PromptBuilder` implementation
- `agent-core/src/prompt/mutation.rs` — `PromptMutation` + `From<String>` + `From<&str>`
- `agent-core/src/prompt/builder_tests.rs` — unit tests (or inline `#[cfg(test)]` module)
- `agent-core/src/prompt/mutation_tests.rs` — unit tests (or inline `#[cfg(test)]` module)

**Acceptance Criteria**:
- [ ] `PromptBuilder::from_base("...").render()` returns `"..."`
- [ ] `upsert_fragment` replaces by id, sorts by priority (stable)
- [ ] `remove_by_source` / `remove_by_kind` / `remove_by_id` work correctly
- [ ] `estimate_tokens` uses `chars / 4.0` heuristic
- [ ] `apply_mutation` handles all `PromptMutation` fields in correct order
- [ ] `From<String> for PromptBuilder` produces a single-fragment builder (BasePersona)
- [ ] Unit tests cover edge cases: empty builder, duplicate ids, negative priorities

**Estimated Effort**: 1-2 sessions

---

### M2: SessionActor Internal PromptBuilder Integration (Phase 1)
**Scope**: Replace `system_prompt: String` storage with `PromptBuilder` in `SessionActor`. AgentLoopConfig 继续使用 `Option<String>` 传递渲染后的 prompt，Hook mutation 类型保持不变（Phase 2 再改）。

**Files**:
- `agent-core/src/harness/session.rs` — `SessionActor` field + `set_system_prompt` + `system_prompt()` refactor
- `agent-core/src/harness/agent_loop.rs` — `AgentLoopConfig` remove `skills` field; `run_turn()` remove skills concatenation
- `tenant/src/manager.rs` — update `system_prompt()` call sites (return type changes from `&str` to `String`)

**完整影响文件清单**：
- `agent-core/src/harness/session.rs` — SessionActor 结构 + 方法
- `agent-core/src/harness/agent_loop.rs` — AgentLoopConfig 去 skills、run_turn 去拼接、测试 helper 更新
- `tenant/src/manager.rs` — SessionInfo 存储时调用 `actor.system_prompt()`

**行为变化（需测试验证）**：
- `on_before_agent_start` / `on_before_provider_request` 返回 raw string override 时，**不再保留 skills XML**（因为 skills 已在 SessionActor 中渲染进 builder，AgentLoop 只做字符串替换）
- 这与旧行为不同：旧代码在 `run_turn()` 中拼接 skills，所以即使 Hook 覆盖了 system_prompt，skills 仍会追加

**Acceptance Criteria**:
- [ ] `SessionActor::new` 签名不变（仍接受 `String`），内部创建 `PromptBuilder::from_base`
- [ ] `SessionActor::set_system_prompt` 重建 PromptBuilder，保留 SkillsDirectory fragment
- [ ] `SessionActor::system_prompt()` 返回 `String`（`builder.render()`），不再是 `&str`
- [ ] `AgentLoopConfig` 移除 `skills: Vec<Skill>` 字段
- [ ] `AgentLoop::run_turn()` 移除 skills XML 拼接，直接使用传入的 `system_prompt: Option<String>`
- [ ] 所有调用 `system_prompt()` 的地方适配返回类型变化
- [ ] Existing tests compile and pass

**Dependencies**: M1

**Estimated Effort**: 1 session

---

### M3: Skills Injection Refactor
**Scope**: Move skills XML injection from string concatenation in `run_turn()` to `PromptBuilder::upsert_fragment`.

**Files**:
- `agent-core/src/harness/agent_loop.rs` — remove inline skills concatenation
- `agent-core/src/skills/injector.rs` — no change to `format_skills_for_prompt()`

**Acceptance Criteria**:
- [ ] `AgentLoop::run_turn()` upserts `SkillsDirectory` fragment into `turn_builder`
- [ ] `turn_builder.render_option()` produces semantically equivalent output to old concatenation logic (empty → `None`, non-empty → `Some(text)`)
- [ ] `skills_integration_tests.rs` pass without modification

**Dependencies**: M2

**Estimated Effort**: 0.5 session

---

### M4: Hook Mutation Types + HookRouter (Phase 2)
**Scope**: Update mutation structs and HookRouter to expose `PromptBuilder` to Extensions.

**Files**:
- `agent-core/src/hook/mutations.rs` — add `prompt_mutation: Option<PromptMutation>` to `BeforeAgentStartMutation` and `ProviderRequestMutation`; keep `system_prompt` for backward compat
- `agent-core/src/hook/context.rs` — add `prompt_builder: PromptBuilder` to `BeforeAgentStartCtx` and `ProviderRequestCtx`; keep `system_prompt: Option<String>` for render result
- `extensions/src/host/hook_router.rs` — chain-merge logic: clone builder per Extension
- `agent-core/src/prompt/mutation.rs` — add `impl From<Option<String>> for Option<PromptBuilder>`

**Acceptance Criteria**:
- [ ] `BeforeAgentStartMutation` has `prompt_mutation: Option<PromptMutation>` + `system_prompt: Option<String>` (legacy)
- [ ] `ProviderRequestMutation` has `prompt_mutation: Option<PromptMutation>` + `system_prompt: Option<Option<String>>` (legacy)
- [ ] `BeforeAgentStartCtx` has `prompt_builder: PromptBuilder` + `system_prompt: Option<String>`
- [ ] `ProviderRequestCtx` has `prompt_builder: PromptBuilder` + `system_prompt: Option<String>`
- [ ] `HookRouter` chain-merge passes builder clone to each Extension
- [ ] `impl From<Option<String>> for Option<PromptBuilder>` exists
- [ ] Existing Extension implementations compile with **minimal** changes (`.into()` → `PromptBuilder::from()`)

**Dependencies**: M2 (Phase 1 complete)

**Estimated Effort**: 1-2 sessions

---

### M5: Tenant Manager Initialization
**Scope**: Update `TenantManagerImpl::create_session()` to initialize `PromptBuilder`.

**Files**:
- `tenant/src/manager.rs` — create builder from default/param, optionally inject `TenantContext`

**Acceptance Criteria**:
- [ ] `create_session` builds `PromptBuilder::from_base(system_prompt)`
- [ ] `TenantContext` fragment injected if tenant metadata available
- [ ] `tenant/tests/manager.rs` pass

**Dependencies**: M2

**Estimated Effort**: 0.5 session

---

### M6: Test Fixes + Integration Tests
**Scope**: Fix all compilation errors in tests and add new integration tests.

**Files**:
- `agent-core/tests/loop_integration_tests.rs` — update `make_loop_config` helper (remove skills field)
- `agent-core/tests/hook_dispatcher_tests.rs` — verify hook override behavior (skills lost after raw string override)
- `agent-core/tests/skills_integration_tests.rs` — verify skills render correctly via PromptBuilder
- `extensions/tests/integration_agent_loop.rs` — update test helpers
- `tenant/tests/integration.rs` — update session creation calls (if any direct system_prompt access)
- `AGENTS.md` — update current status table

**New Tests**:
- `agent-core/src/harness/session.rs` (inline `#[cfg(test)]`) — `SessionActor` with skills: `system_prompt()` contains `<available_skills>`
- `agent-core/src/harness/session.rs` — `set_system_prompt()` preserves SkillsDirectory fragment
- `agent-core/tests/prompt_builder_integration_tests.rs` — end-to-end: SessionActor → AgentLoopConfig → AgentLoop → LLM context contains expected prompt

**Acceptance Criteria**:
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace` passes
- [ ] `SessionActor::system_prompt()` with skills returns string containing `<available_skills>`
- [ ] `set_system_prompt()` rebuilds BasePersona but keeps SkillsDirectory
- [ ] Hook raw-string override replaces entire prompt (including skills) — verify with test

**Dependencies**: M1-M3 (Phase 1 only); M4-M5 for Phase 2

**Estimated Effort**: 1-2 sessions (Phase 1 only)

---

## 2. 实施顺序

### Phase 1（当前实施）

```
M1 ──→ M2 ──→ M3 ──→ M6
```

**推荐顺序**：
1. M1（已 ✅ 完成）
2. M2（SessionActor 内部 PromptBuilder 化）
3. M3（AgentLoop 移除 skills 拼接）
4. M6（测试修复 + AGENTS.md 更新）

### Phase 2（后续）

```
M4 ──→ M5 ──→ M6 扩展
```

---

## 3. 风险评估

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| 改动面过大，测试修复耗时超预期 | 中 | 高 | M1-M5 各自独立分支，逐步合并；优先保证 `cargo test` 在每一步都通过 |
| `From<String>` 自动转换引入类型推断歧义 | 低 | 中 | 显式标注类型或使用 `PromptBuilder::from_base()` 替代 blanket `From` |
| Hook override 后 skills 丢失导致 Extension 行为变化 | 中 | 中 | 在 Phase 1 文档中明确标注此行为；Phase 2 引入 `prompt_mutation` 后恢复精准修改能力 |
| Extension 作者对 `PromptMutation` 概念困惑 | 低 | 中 | 文档 + 示例 Extension（`examples/prompt-mutating-extension.rs`） |
| `PromptBuilder` clone 成本影响性能 | 低 | 低 | 每个 turn 仅 clone 一次（< 10 fragments）；使用 `Arc<str>` 优化未来可单独做 |

---

## 4. 分支策略

建议单分支 `feat/prompt-builder` 顺序实施。M1-M4 每完成一个里程碑即 commit：`feat(prompt-builder): M1 core types` 等。

---

## 5. 完成标准

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace` passes
- [ ] `cargo doc --workspace` builds without warnings
- [ ] `docs/specs/prompt-builder.md` 中 Phase 1 目标实现（G1/G3/G4 部分）
- [ ] AGENTS.md 当前状态表更新：PromptBuilder 标记为 "✅ 已接入运行时代码（Phase 1）"
- [ ] `render_option()` 语义验证：空 builder → `None`，非空 → `Some(text)`
