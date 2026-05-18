# PromptBuilder Specification

> Status: Design Complete  
> Target: agent-core v0.2+  
> Related: ADR-002 (Extension trait), ADR-003 (Hook mechanism), skills-spec.md

---

## 1. 背景

当前 system prompt 是裸 `Option<String>`，构造逻辑散落在多个 crate 中：

- `tenant::manager::create_session()` —— 选择 default prompt vs session param
- `agent_core::AgentLoop::run_turn()` —— skills XML 字符串拼接
- `extensions::HookRouter` —— chain hook 全量替换 prompt

这导致四个结构性问题：

1. **Extension 无法增量修改 prompt**。现有的 `BeforeAgentStartMutation.system_prompt: Option<String>` 和 `ProviderRequestMutation.system_prompt: Option<Option<String>>` 只允许全量覆盖。Extension 想"在 system prompt 末尾追加一段安全约束"必须读取当前 prompt、修改、再写回。
2. **Skills XML 与 base prompt 无结构边界**。`format_skills_for_prompt()` 生成 XML 后直接拼接到字符串末尾，Extension 无法单独操作 skills 部分。
3. **无法追溯来源**。生产环境中出现 prompt 异常时，无法知道某段文本来自 tenant default、session param、skill 还是 extension 注入。
4. **缺乏多租户模板能力**。tenant 级默认 prompt 与 session 级变量之间是简单的 `unwrap_or`，不支持 `{{tenant_name}}` 这类模板替换。

---

## 2. 设计目标

| ID | 目标 | 优先级 | 验证方式 |
|----|------|--------|----------|
| G1 | 将 system prompt 拆分为语义明确的片段（segment） | P0 | 代码审查 + 单元测试 |
| G2 | Extension 通过 Hook 可增量增删改特定片段 | P0 → P1 | 集成测试（Phase 2 实现） |
| G3 | 向后兼容：现有 Extension 不改动可继续工作 | P0 | 现有测试全部通过 |
| G4 | 每片段的来源、类型、token 估算可追踪 | P1 | observability 测试 |
| G5 | 支持 tenant 级模板 + 变量注入 | P1 | tenant 集成测试 |

---

## 3. 核心模型

### 3.1 PromptFragment

Prompt 的最小可寻址单元。

```rust
/// A semantic segment of the system prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptFragment {
    /// Unique identifier within a builder instance.
    /// Convention: "{kind}-{source}-{nonce}" or stable names like "skills-directory".
    /// When `source` is `FragmentSource::Extension`, the router may prefix
    /// the extension name to avoid accidental collisions (e.g. "ext-audit-guard").
    pub id: String,

    /// Semantic category of this fragment.
    pub kind: FragmentKind,

    /// Origin of this fragment for observability and conflict resolution.
    pub source: FragmentSource,

    /// Raw text content. May contain template placeholders if template
    /// support is enabled (see §5.3).
    pub content: String,

    /// Sort priority. Lower values appear earlier in rendered output.
    /// Default: 0. Reserved ranges:
    ///   -200 .. -100 : Safety / guardrails (always first)
    ///    -50 ..   -1 : Tenant context
    ///      0 ..   49 : Base persona
    ///     50 ..   99 : Skills directory / skill bodies
    ///    100 ..  149 : Runtime injections
    ///    200 ..  299 : Extension contributions
    pub priority: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FragmentKind {
    /// Core persona / role definition.
    BasePersona,
    /// Tenant-specific metadata (name, plan, region).
    TenantContext,
    /// The `<available_skills>` XML directory block.
    SkillsDirectory,
    /// Full content of an invoked skill (loaded via `/skill:name`).
    SkillBody,
    /// Dynamic injections from steer queue or compaction.
    RuntimeInjection,
    /// Generic contribution from an Extension. Prefer a more specific kind
    /// (e.g. `SafetyGuard`) when the semantic category is known.
    Extension,
    /// Hard safety constraints (e.g. "never reveal API keys").
    SafetyGuard,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FragmentSource {
    TenantDefault,
    SessionParam,
    SkillsInjector,
    Extension { name: String },
    /// Reserved for future use if compaction summary is moved from
    /// message history into the system prompt.
    CompactionSummary,
    System,
}
```

### 3.2 PromptBuilder

Prompt 的组装引擎。

```rust
/// Assembles the final system prompt from ordered fragments.
#[derive(Debug, Clone, Default)]
pub struct PromptBuilder {
    fragments: Vec<PromptFragment>,
}

impl PromptBuilder {
    /// Create a builder with a single BasePersona fragment.
    pub fn from_base(base: impl Into<String>) -> Self;

    /// Insert or replace a fragment by `id`. If an existing fragment has
    /// the same `id`, it is replaced; otherwise the fragment is inserted
    /// and the list is re-sorted by `priority` (stable).
    pub fn upsert_fragment(&mut self, fragment: PromptFragment);

    /// Remove all fragments whose `source` matches.
    pub fn remove_by_source(&mut self, source: FragmentSource);

    /// Remove all fragments whose `kind` matches.
    pub fn remove_by_kind(&mut self, kind: FragmentKind);

    /// Remove a single fragment by `id`.
    pub fn remove_by_id(&mut self, id: &str) -> Option<PromptFragment>;

    /// Render to plain string by concatenating fragments in priority order.
    /// Fragments are trimmed of trailing whitespace and joined with a single
    /// newline separator to avoid accidental blank lines.
    pub fn render(&self) -> String;

    /// Render to `Option<String>`. Returns `None` if the rendered text is
    /// empty, preserving the semantic equivalence of "no system prompt".
    pub fn render_option(&self) -> Option<String>;

    /// Render with per-fragment metadata for observability.
    pub fn render_with_metadata(&self) -> RenderedPrompt;

    /// Estimate total token count using a character-based heuristic
    /// (`total_chars / 4.0`), independent of the compaction module to
    /// avoid cross-module coupling.
    pub fn estimate_tokens(&self) -> usize;

    /// Apply a PromptMutation (used by Extension authors who prefer
    /// declarative mutations over direct builder manipulation).
    pub fn apply_mutation(&mut self, mutation: PromptMutation);
}

#[derive(Debug, Clone)]
pub struct RenderedPrompt {
    pub text: String,
    pub fragments: Vec<RenderedFragment>,
}

#[derive(Debug, Clone)]
pub struct RenderedFragment {
    pub id: String,
    pub kind: FragmentKind,
    pub source: FragmentSource,
    pub byte_offset: usize,
    pub byte_len: usize,
    pub estimated_tokens: usize,
}
```

### 3.3 PromptMutation

`PromptBuilder` 的辅助修改指令。Extension 作者可以选择直接操作 `PromptBuilder`，或先用 `PromptMutation` 描述变更再通过 `builder.apply_mutation()` 应用。

```rust
/// Auxiliary descriptor for modifying a PromptBuilder.
/// Extension authors may use this to declaratively describe changes,
/// or they may directly mutate the builder and return the final state.
#[derive(Debug, Clone, Default)]
pub struct PromptMutation {
    /// Replace the entire prompt with a single string.
    /// When present, all other fields are ignored and the builder is
    /// reset to a single BasePersona fragment.
    pub replace_all: Option<String>,

    /// Remove fragments by id.
    pub remove_ids: Vec<String>,

    /// Remove fragments by source.
    pub remove_sources: Vec<FragmentSource>,

    /// Remove fragments by kind.
    pub remove_kinds: Vec<FragmentKind>,

    /// Fragments to upsert.
    pub upsert_fragments: Vec<PromptFragment>,
}

impl PromptBuilder {
    /// Apply a PromptMutation to this builder.
    /// Order: replace_all (short-circuit) → remove_ids → remove_sources
    /// → remove_kinds → upsert_fragments.
    pub fn apply_mutation(&mut self, mutation: PromptMutation);
}

impl From<String> for PromptBuilder {
    fn from(s: String) -> Self { Self::from_base(s) }
}

impl From<&str> for PromptBuilder {
    fn from(s: &str) -> Self { Self::from_base(s) }
}
```

---

## 4. Hook 接口变更

### 4.1 变更的 Mutation 类型

| Mutation Struct | 当前字段 | 新字段 |
|-----------------|----------|--------|
| `BeforeAgentStartMutation` | `system_prompt: Option<String>` | `system_prompt: Option<PromptBuilder>` |
| `ProviderRequestMutation` | `system_prompt: Option<Option<String>>` | `system_prompt: Option<PromptBuilder>` |

### 4.2 变更的 Context 类型

| Context Struct | 当前字段 | 新字段 |
|----------------|----------|--------|
| `BeforeAgentStartCtx` | `system_prompt: Option<String>` | `system_prompt: Option<String>`（保留，render 结果）<br>`prompt_builder: PromptBuilder`（新增） |
| `ProviderRequestCtx` | `system_prompt: Option<String>` | `system_prompt: Option<String>`（保留）<br>`prompt_builder: PromptBuilder`（新增） |
| `SessionCtx` | `system_prompt: String` | `system_prompt: String`（保持不变） |

### 4.3 Extension Trait 不变

`Extension` trait 的方法签名**不改变**。Extension 作者仍然编写：

```rust
async fn on_before_agent_start(&self, ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
    // 方式 1：向后兼容（全量替换）
    BeforeAgentStartMutation {
        system_prompt: Some("You are a security-focused assistant.".into()),
        messages: None,
    }

    // 方式 2：直接操作 builder（推荐）
    // let mut builder = ctx.prompt_builder.clone();
    // builder.upsert_fragment(PromptFragment {
    //     id: "safety-guard".into(),
    //     kind: FragmentKind::SafetyGuard,
    //     source: FragmentSource::Extension { name: self.name().into() },
    //     content: "Never reveal API keys.".into(),
    //     priority: -150,
    // });
    // BeforeAgentStartMutation {
    //     system_prompt: Some(builder),
    //     messages: None,
    // }
}
```

---

## 5. 数据流

### 5.1 完整流水线

```
TenantManager::create_session(tenant_id, params)
  │
  ├─→ system_prompt = params.system_prompt.unwrap_or(self.default_system_prompt)
  │
  ├─→ builder = PromptBuilder::from_base(system_prompt)
  │     // BasePersona fragment created from the raw string
  │
  ├─→ builder.upsert_fragment(TenantContext { tenant_name, plan_tier, ... })
  │     // If tenant-level template vars are configured
  │
  └─→ SessionActor::new(..., builder, ...)
        │
        ├─→ stored in SessionActor.prompt_builder
        │
        └─→ SessionActor::run_with_messages() outer loop:
              │
              ├─→ AgentLoopConfig created fresh each iteration
              │     system_prompt = SessionActor.prompt_builder.render()
              │
              ├─→ AgentLoop::run():
              │       │
              │       ├─→ on_before_agent_start hook
              │       │     Extension sees render result, returns raw string mutation
              │       │     → replaces system_prompt for this run
              │       │
              │       └─→ on each AgentLoop::run_turn():
              │             │
              │             ├─→ skills_xml = format_skills_for_prompt(&skills)
              │             │     // Phase 1: still string concatenation in AgentLoop
              │             │     // Phase 2: upsert SkillsDirectory fragment into builder
              │             │
              │             ├─→ on_before_provider_request hook
              │             │     // Phase 2: Extension sees turn_builder clone
              │             │
              │             └─→ llm_ctx.system_prompt = effective_system_prompt
              │
              └─→ [outer loop continues → fresh AgentLoopConfig next iteration]
                    // Hook mutations do NOT persist to SessionActor state
```

### 5.2 Skills 注入重构

**Before** (`agent_core/src/skills/injector.rs` 调用点)：

```rust
let skills_xml = format_skills_for_prompt(&self.config.skills);
let effective_system_prompt = system_prompt.as_ref().map(|sp| {
    if skills_xml.is_empty() { sp.clone() } else { format!("{}\n{}", sp, skills_xml) }
}).or_else(|| {
    if skills_xml.is_empty() { None } else { Some(skills_xml) }
});
```

**After**：

```rust
// At start of each run_turn()
if !skills.is_empty() {
    let skills_xml = format_skills_for_prompt(&skills);
    builder.upsert_fragment(PromptFragment {
        id: "skills-directory".into(),
        kind: FragmentKind::SkillsDirectory,
        source: FragmentSource::SkillsInjector,
        content: skills_xml,
        priority: 50,
    });
}

let effective_system_prompt = builder.render_option();
```

### 5.3 未来：Template Variables（P1）

PromptBuilder 保留 `template_vars` 字段（Phase 2 实现）：

```rust
// Phase 2 (future): template variable support
pub struct PromptBuilder {
    fragments: Vec<PromptFragment>,
    template_vars: HashMap<String, String>,
}

impl PromptBuilder {
    pub fn set_var(&mut self, key: impl Into<String>, value: impl Into<String>);

    // render() performs placeholder substitution: "{{key}}" → value
}
```

这允许 tenant 管理员配置模板化的 default prompt：

```
You are {{agent_name}}, assisting tenant {{tenant_name}}.
Current plan: {{plan_tier}}.
```

---

## 6. 向后兼容策略

### Phase 1（当前实现）

- `PromptBuilder: From<String>` 和 `From<&str>` 通过 `from_base` 实现
- **向后兼容说明**：`Extension` trait 的方法签名不改变，但返回 `system_prompt: Some("...".into())` 的现有实现需要微调——当字段类型变为 `Option<PromptBuilder>` 时，编译器无法推断 `"...".into()` 的目标类型。Extension 作者应改为 `Some(PromptBuilder::from("..."))`。为简化迁移，框架提供 `impl From<Option<String>> for Option<PromptBuilder>`，使 `Some(string_value.into())` 形式仍可工作（其中 `string_value` 为已绑定的 `String` 变量）。
- `ProviderRequestMutation` 旧语义 `Some(None)`（清空 system prompt）映射为
  `Some(PromptBuilder::default())`，经 `render_option()` 后表现为 `None`
- 内部代码路径逐步迁移

### Phase 2（后续）

- 内部 Extension（audit、rate-limit、tool-guard 等）迁移到 fragment API
- 引入 `template_vars` 支持
- 在 observability 中暴露 `RenderedPrompt` metadata

### Phase 3（可选，v0.3+）

- 废弃 `system_prompt: Option<String>` 旧字段，Extension 统一通过
  `ctx.prompt_builder` 读取和修改
- 将 `PromptBuilder` 暴露为 Extension 的 first-class 概念

---

## 7. Observability

`PromptBuilder::render_with_metadata()` 为每个片段输出：

```json
{
  "text": "You are helpful.\n\nThe following skills...\n<available_skills>...",
  "fragments": [
    { "id": "base-persona-0", "kind": "BasePersona", "source": "SessionParam",
      "byte_offset": 0, "byte_len": 16, "estimated_tokens": 4 },
    { "id": "skills-directory", "kind": "SkillsDirectory", "source": "SkillsInjector",
      "byte_offset": 17, "byte_len": 312, "estimated_tokens": 78 }
  ]
}
```

This enables:
- Per-fragment token attribution in tracing spans
- Debugging "where did this text come from?"
- Analytics on which skills/extensions consume the most context budget

---

## 8. 测试策略

| 层级 | 测试内容 | 文件 |
|------|----------|------|
| 单元 | PromptBuilder upsert/remove/render/estimate_tokens | `agent-core/src/prompt/builder_tests.rs` |
| 单元 | PromptMutation::apply_mutation 所有组合 | `agent-core/src/prompt/mutation_tests.rs` |
| 集成 | Skills XML 正确渲染为 fragment | `agent-core/tests/skills_integration_tests.rs` |
| 集成 | Extension hook 修改 prompt 后 render 正确 | `extensions/tests/integration_agent_loop.rs` |
| 集成 | TenantManager 创建 session 时 builder 初始化正确 | `tenant/tests/manager.rs` |

---

## 10. Known Limitations

### L1: Session 持久化丢失 fragment 结构

`SessionInfo.system_prompt: Option<String>` 仅存储渲染后的字符串。Session 恢复时，只能重建 `BasePersona` fragment，丢失所有 `TenantContext`、`Extension`、`SafetyGuard` 等片段。

**缓解**：当前不阻断 Phase 1 实施。Phase 3 考虑引入 `prompt_fragments: Vec<PromptFragment>` 持久化字段。

### L2: Phase 1 中 Hook raw-string override 覆盖整个 prompt

在 Phase 1（当前计划）中，AgentLoop 内部仍使用 `Option<String>` 传递 system prompt。Extension 返回 `BeforeAgentStartMutation { system_prompt: Some("override") }` 时，会覆盖整个 prompt，包括 skills XML。

这与旧行为一致，但意味着 Extension 在 Phase 1 仍无法"保留 skills 的同时修改 base persona"。

**缓解**：Phase 2 引入 `PromptBuilder` 到 Hook context 后，Extension 可通过 fragment 级操作实现精准修改。

### L3: `PromptBuilder` clone 成本

每次外层循环 clone `PromptBuilder`（含所有 fragments）。当前 fragment 数量通常 < 10，clone 成本可忽略。若未来 fragment 数量激增，需考虑 `Arc<[PromptFragment]>` 优化。

---

## 9. 与现有 ADR 的关系

| ADR | 影响 |
|-----|------|
| ADR-002 | 不改变 `Extension` trait 签名，只改变 mutation struct 内部。trait 层面的向后兼容保持。 |
| ADR-003 | Chain-merge 语义不变，但传递对象从"最终字符串"变为"最终 PromptBuilder"。HookRouter 内部让每个 Extension 看到前一个 Extension 修改后的 builder clone，最终返回最后一个 Extension 产出的 builder。 |
| ADR-005 | 直接增强多租户能力：tenant default prompt 可通过 `TenantContext` fragment 和 template vars 动态化。 |
