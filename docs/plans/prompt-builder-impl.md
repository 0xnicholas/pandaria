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

### M2: AgentLoopConfig / SessionActor Integration
**Scope**: Replace `system_prompt: String` storage with `PromptBuilder` in `SessionActor` and `AgentLoopConfig`.

**Files**:
- `agent-core/src/harness/session.rs` — `SessionActor` field + `set_system_prompt` refactor
- `agent-core/src/harness/agent_loop.rs` — `AgentLoopConfig` field change

**Acceptance Criteria**:
- [ ] `SessionActor::new` accepts `PromptBuilder` instead of `String`
- [ ] `SessionActor::set_system_prompt` converts `String` → `PromptBuilder::from_base`
- [ ] `AgentLoopConfig` stores `prompt_builder: PromptBuilder`
- [ ] `AgentLoop::run()` clones config builder, replaces it with `on_before_agent_start`
  returned builder if present, and persists the final builder back into config
  for all turns within this `AgentLoop::run()`
- [ ] `AgentLoop::run_turn()` creates `turn_builder` from config builder,
  applies skills + `on_before_provider_request` result,
  and uses `turn_builder.render_option()` for LLM call
- [ ] Existing `SessionActor` / `AgentLoop` tests compile and pass

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

### M4: Hook Mutation Types + HookRouter
**Scope**: Update mutation structs and HookRouter to use `PromptBuilder`.

**Files**:
- `agent-core/src/hook/mutations.rs` — update `BeforeAgentStartMutation.system_prompt: Option<PromptBuilder>`, `ProviderRequestMutation.system_prompt: Option<PromptBuilder>`
- `agent-core/src/hook/context.rs` — add `prompt_builder: PromptBuilder` to `BeforeAgentStartCtx` and `ProviderRequestCtx`; keep `system_prompt: Option<String>` for backward compat
- `extensions/src/host/hook_router.rs` — chain-merge logic: clone builder per Extension, return final builder

**Acceptance Criteria**:
- [ ] `BeforeAgentStartMutation.system_prompt: Option<PromptBuilder>`
- [ ] `ProviderRequestMutation.system_prompt: Option<PromptBuilder>`
- [ ] `BeforeAgentStartCtx` has `prompt_builder: PromptBuilder` + `system_prompt: Option<String>`
- [ ] `ProviderRequestCtx` has `prompt_builder: PromptBuilder` + `system_prompt: Option<String>`
- [ ] `HookRouter::on_before_agent_start` passes builder clone to each Extension, returns final builder
- [ ] `HookRouter::on_before_provider_request` passes builder clone to each Extension, returns final builder
- [ ] Existing Extension implementations (builtins) compile without changes

**Dependencies**: M1

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
- `agent-core/tests/loop_integration_tests.rs` — update `make_loop_config` helper
- `agent-core/tests/hook_dispatcher_tests.rs` — update mock dispatcher returns
- `agent-core/tests/skills_integration_tests.rs` — verify no regressions
- `extensions/tests/integration_agent_loop.rs` — update test helpers
- `tenant/tests/integration.rs` — update session creation calls
- `AGENTS.md` — update current status table

**New Tests**:
- `agent-core/src/prompt/builder_tests.rs` — comprehensive builder unit tests
- `agent-core/src/prompt/mutation_tests.rs` — mutation application tests
- `agent-core/tests/prompt_builder_integration_tests.rs` — end-to-end prompt assembly

**Acceptance Criteria**:
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace` passes
- [ ] New integration test verifies: Extension adds fragment → render contains it → fragment metadata correct

**Dependencies**: M1-M5

**Estimated Effort**: 2-3 sessions

---

## 2. 实施顺序

```
M1 ──→ M2 ──→ M3 ──┐
  │                 ├──→ M6
  └──→ M4 ──→ M5 ──┘
```

**推荐顺序**：
1. M1（独立，可并行于其他工作）
2. M2 + M4 并行（SessionActor 集成 与 Hook 类型/Router 互不依赖）
3. M3 + M5 并行（Skills 重构 与 TenantManager 初始化互不依赖）
4. M6（集中测试修复 + AGENTS.md 更新）

---

## 3. 风险评估

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| 改动面过大，测试修复耗时超预期 | 中 | 高 | M1-M5 各自独立分支，逐步合并；优先保证 `cargo test` 在每一步都通过 |
| `From<String>` 自动转换引入类型推断歧义 | 低 | 中 | 显式标注类型或使用 `PromptBuilder::from_base()` 替代 blanket `From` |
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
- [ ] `docs/specs/prompt-builder.md` 中所有 P0 目标实现
- [ ] AGENTS.md 当前状态表更新：PromptBuilder 标记为 ✅
- [ ] `render_option()` 语义验证：空 builder → `None`，非空 → `Some(text)`
