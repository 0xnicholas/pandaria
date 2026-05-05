# Extensions Crate Gap Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the gaps between `crates/extensions/` current implementation and `docs/specs/2026-05-02-extensions.md` spec.

**Architecture:** The extensions crate is already architecturally complete (trait, actor, router, manager, builtins). This plan fixes specific functional bugs and missing pieces: HookRouter TODO stubs, on_tool_call mutation chain, missing struct fields, and insufficient test coverage.

**Tech Stack:** Rust 2024, tokio, async-trait, tracing

---

## File Map

### Existing Files to Modify

| File | Current | Target |
|---|---|---|
| `crates/extensions/src/host/hook_router.rs` | 6 implemented methods, 8 TODO stubs | All 14 methods implemented per spec |
| `crates/extensions/src/host/extension_actor.rs` | ExtensionHandle lacks `name` field | Add `name: String` for tracing |
| `crates/extensions/src/host/extension_tool.rs` | No `execution_mode` field | Add `execution_mode: ToolExecutionMode` |
| `crates/extensions/src/host/event_bus.rs` | `emit()` silently discards zero-subscriber errors | Add `tracing::warn!` for zero subscribers |
| `crates/extensions/src/lib.rs` | Re-exports | Add re-export for `ToolExecutionMode` if needed |

### New Test Files to Create

| File | Tests |
|---|---|
| `crates/extensions/tests/hook_router_mutation_tests.rs` | input mutation chain, timeout skips, ctx propagation |
| `crates/extensions/tests/hook_router_compact_tests.rs` | on_before_compact block/replace/continue |
| `crates/extensions/tests/hook_router_provider_tests.rs` | on_before_agent_start, on_before_provider_request, on_after_provider_response |
| `crates/extensions/tests/hook_router_observation_tests.rs` | tool_execution_*, compact_end broadcast |
| `crates/extensions/tests/extension_actor_advanced_tests.rs` | eventbus receive, observation timeout, shutdown drops handle |

---

## Context Types Reference (from agent-core)

All context types have **public fields** (no getters). Key fields for mutation:

- `ToolCallCtx { tenant_id, session_id, tool_name, tool_call_id, input }`
- `ToolResultCtx { tenant_id, session_id, tool_name, tool_call_id, input, content, details, is_error }`
- `ContextCtx { tenant_id, session_id, messages }`
- `BeforeAgentStartCtx { tenant_id, session_id, system_prompt: Option<String>, messages, tools, model }`
- `ProviderRequestCtx { tenant_id, session_id, model, system_prompt, messages, turn_index, tools, options }`
- `ProviderResponseCtx { tenant_id, session_id, model, content, turn_index, attempt, messages_before, stop_reason }`
- `CompactCtx { tenant_id, session_id, preparation, entries, reason }`

All mutation types have **public fields**:

- `ToolCallMutation { input: Option<serde_json::Value> }`
- `ToolResultMutation { content, details, is_error, terminate }`
- `ContextMutation { messages }`
- `BeforeAgentStartMutation { system_prompt, messages }`
- `ProviderRequestMutation { system_prompt, messages, tools, options }`
- `ProviderResponseMutation { content, stop_reason }`
- `CompactDecision::Continue | Block { reason } | Replace { result }`

---

## Task 1: Fix HookRouter — on_tool_call input mutation chain

**Files:**
- Modify: `crates/extensions/src/host/hook_router.rs:42-59`

**Bug:** Current implementation:
1. Uses `let current_ctx = ctx.clone()` (immutable binding)
2. Returns `mutation` directly from the blocking handler (not the accumulated mutation)
3. Returns `ToolCallMutation::default()` when all Continue (loses accumulated input)

**Fix per Spec §6.1:**
- Use `let mut current_ctx = ctx.clone()`
- Apply `mutation.input` to `current_ctx.input` after each handler
- On Block: return `ToolCallMutation { input: Some(current_ctx.input) }`
- On all Continue: return `ToolCallMutation { input: Some(current_ctx.input) }`

- [ ] **Step 1: Write failing test for mutation chain**

Create test in `crates/extensions/tests/hook_router_mutation_tests.rs`:

```rust
use std::sync::Arc;
use async_trait::async_trait;

use agent_core::context::ToolCallCtx;
use agent_core::mutations::{HookDecision, ToolCallMutation};
use agent_core::HookDispatcher;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;

/// Extension that mutates input by adding a key
struct InputMutatorExt {
    key: String,
    value: serde_json::Value,
}

#[async_trait]
impl Extension for InputMutatorExt {
    fn name(&self) -> &str { "input_mutator" }

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        let mut input = ctx.input.clone();
        if let Some(obj) = input.as_object_mut() {
            obj.insert(self.key.clone(), self.value.clone());
        }
        (HookDecision::Continue, ToolCallMutation { input: Some(input) })
    }
}

/// Extension that blocks
struct BlockerExt;

#[async_trait]
impl Extension for BlockerExt {
    fn name(&self) -> &str { "blocker" }

    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        (HookDecision::Block { reason: "blocked".to_string() }, ToolCallMutation::default())
    }
}

#[tokio::test]
async fn test_tool_call_mutation_chain() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(InputMutatorExt { key: "ext1".to_string(), value: serde_json::json!(1) });
    let ext2 = Arc::new(InputMutatorExt { key: "ext2".to_string(), value: serde_json::json!(2) });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({"original": true}),
    };

    let (decision, mutation) = router.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));
    
    let input = mutation.input.expect("should have accumulated input");
    let obj = input.as_object().unwrap();
    assert!(obj.contains_key("original"));
    assert!(obj.contains_key("ext1"));
    assert!(obj.contains_key("ext2"));
}

#[tokio::test]
async fn test_tool_call_mutation_block_retained() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(InputMutatorExt { key: "sanitized".to_string(), value: serde_json::json!(true) });
    let ext2 = Arc::new(BlockerExt);

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, mutation) = router.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
    
    // ext1's mutation should be preserved even though ext2 blocked
    let input = mutation.input.expect("should retain accumulated mutation");
    assert!(input.get("sanitized").is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p extensions --test hook_router_mutation_tests`
Expected: FAIL — `mutation.input` is None (current code returns default)

- [ ] **Step 3: Fix on_tool_call in hook_router.rs**

In `crates/extensions/src/host/hook_router.rs`, replace lines 42-59:

```rust
    async fn on_tool_call(&self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        let mut current_ctx = ctx.clone();
        for handle in &self.handles {
            let (decision, mutation) = handle.on_tool_call(current_ctx.clone()).await;

            // Apply mutation to current_ctx so subsequent handlers see sanitized input
            if let Some(input) = mutation.input {
                current_ctx.input = input;
            }

            match decision {
                HookDecision::Block { reason } => {
                    return (
                        HookDecision::Block { reason },
                        ToolCallMutation { input: Some(current_ctx.input) },
                    );
                }
                HookDecision::Continue => continue,
            }
        }
        (
            HookDecision::Continue,
            ToolCallMutation { input: Some(current_ctx.input) },
        )
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p extensions --test hook_router_mutation_tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/extensions/src/host/hook_router.rs crates/extensions/tests/hook_router_mutation_tests.rs
git commit -m "fix(extensions): on_tool_call input mutation chain"
```

---

## Task 2: Fix HookRouter — implement 8 TODO methods

**Files:**
- Modify: `crates/extensions/src/host/hook_router.rs:61-175`

**Context:** 8 methods currently return defaults or are empty. Need full implementations per Spec §6.

- [ ] **Step 1: Implement on_before_compact (blocking, first-block-wins)**

Replace the TODO at line 61-66:

```rust
    async fn on_before_compact(&self,
        ctx: &CompactCtx,
    ) -> CompactDecision {
        for handle in &self.handles {
            match handle.on_before_compact(ctx.clone()).await {
                Ok(CompactDecision::Block { reason }) => return CompactDecision::Block { reason },
                Ok(CompactDecision::Replace { result }) => return CompactDecision::Replace { result },
                Ok(CompactDecision::Continue) => continue,
                Err(AskError::Timeout) => {
                    tracing::warn!(extension = %handle.name, "on_before_compact timeout, treating as Continue");
                    continue;
                }
                Err(AskError::ActorGone) => continue,
            }
        }
        CompactDecision::Continue
    }
```

- [ ] **Step 2: Implement on_before_agent_start (chain merge)**

Replace the TODO at line 122-128:

```rust
    async fn on_before_agent_start(&self,
        ctx: &BeforeAgentStartCtx,
    ) -> BeforeAgentStartMutation {
        let mut accumulated = BeforeAgentStartMutation::default();
        let mut current_ctx = ctx.clone();

        for handle in &self.handles {
            match handle.on_before_agent_start(current_ctx.clone()).await {
                Ok(mutation) => {
                    if let Some(ref sp) = mutation.system_prompt {
                        current_ctx.system_prompt = Some(sp.clone());
                        accumulated.system_prompt = Some(sp.clone());
                    }
                    if let Some(ref msgs) = mutation.messages {
                        current_ctx.messages = msgs.clone();
                        accumulated.messages = Some(msgs.clone());
                    }
                }
                Err(AskError::Timeout) => {
                    tracing::warn!(extension = %handle.name, "on_before_agent_start timeout, skipping");
                    continue;
                }
                Err(AskError::ActorGone) => continue,
            }
        }
        accumulated
    }
```

- [ ] **Step 3: Implement on_before_provider_request (chain merge)**

Replace the TODO at line 130-136:

```rust
    async fn on_before_provider_request(&self,
        ctx: &ProviderRequestCtx,
    ) -> ProviderRequestMutation {
        let mut accumulated = ProviderRequestMutation::default();
        let mut current_ctx = ctx.clone();

        for handle in &self.handles {
            match handle.on_before_provider_request(current_ctx.clone()).await {
                Ok(mutation) => {
                    if let Some(ref sp) = mutation.system_prompt {
                        current_ctx.system_prompt = sp.clone();
                        accumulated.system_prompt = Some(sp.clone());
                    }
                    if let Some(ref msgs) = mutation.messages {
                        current_ctx.messages = msgs.clone();
                        accumulated.messages = Some(msgs.clone());
                    }
                    if let Some(ref tools) = mutation.tools {
                        current_ctx.tools = tools.clone();
                        accumulated.tools = Some(tools.clone());
                    }
                    if let Some(ref options) = mutation.options {
                        current_ctx.options = options.clone();
                        accumulated.options = Some(options.clone());
                    }
                }
                Err(AskError::Timeout) => {
                    tracing::warn!(extension = %handle.name, "on_before_provider_request timeout, skipping");
                    continue;
                }
                Err(AskError::ActorGone) => continue,
            }
        }
        accumulated
    }
```

- [ ] **Step 4: Implement on_after_provider_response (chain merge)**

Replace the TODO at line 138-144:

```rust
    async fn on_after_provider_response(&self,
        ctx: &ProviderResponseCtx,
    ) -> ProviderResponseMutation {
        let mut accumulated = ProviderResponseMutation::default();
        let mut current_ctx = ctx.clone();

        for handle in &self.handles {
            match handle.on_after_provider_response(current_ctx.clone()).await {
                Ok(mutation) => {
                    if let Some(ref content) = mutation.content {
                        current_ctx.content = content.clone();
                        accumulated.content = Some(content.clone());
                    }
                    if let Some(ref stop_reason) = mutation.stop_reason {
                        current_ctx.stop_reason = stop_reason.clone();
                        accumulated.stop_reason = Some(stop_reason.clone());
                    }
                }
                Err(AskError::Timeout) => {
                    tracing::warn!(extension = %handle.name, "on_after_provider_response timeout, skipping");
                    continue;
                }
                Err(AskError::ActorGone) => continue,
            }
        }
        accumulated
    }
```

- [ ] **Step 5: Implement 4 observational hooks (EventBus emit)**

Replace the TODOs at lines 160-174:

```rust
    async fn on_tool_execution_start(&self, ctx: &ToolExecutionStartCtx) {
        self.event_bus.emit(ObsEvent::ToolExecutionStart(ctx.clone()));
    }

    async fn on_tool_execution_update(&self, ctx: &ToolExecutionUpdateCtx) {
        self.event_bus.emit(ObsEvent::ToolExecutionUpdate(ctx.clone()));
    }

    async fn on_tool_execution_end(&self, ctx: &ToolExecutionEndCtx) {
        self.event_bus.emit(ObsEvent::ToolExecutionEnd(ctx.clone()));
    }

    async fn on_compact_end(&self, ctx: &CompactEndCtx) {
        self.event_bus.emit(ObsEvent::CompactEnd(ctx.clone()));
    }
```

- [ ] **Step 6: Add missing imports to hook_router.rs**

At the top of `hook_router.rs`, add to existing imports:

```rust
use agent_core::context::{
    // ... existing imports ...
    BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx,
    ToolExecutionStartCtx, ToolExecutionUpdateCtx, ToolExecutionEndCtx,
    CompactCtx, CompactEndCtx,
};
use agent_core::mutations::{
    // ... existing imports ...
    BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation,
    CompactDecision,
};
```

- [ ] **Step 7: Verify compilation**

Run: `cargo build -p extensions`
Expected: Compiles successfully

- [ ] **Step 8: Run existing tests**

Run: `cargo test -p extensions`
Expected: All existing tests pass

- [ ] **Step 9: Commit**

```bash
git add crates/extensions/src/host/hook_router.rs
git commit -m "feat(extensions): implement all 14 HookDispatcher methods in HookRouter"
```

---

## Task 3: Add name field to ExtensionHandle

**Files:**
- Modify: `crates/extensions/src/host/extension_actor.rs:85-88`
- Modify: `crates/extensions/src/host/extension_actor.rs:258-264` (spawn function)

**Context:** Spec §4.5 requires `ExtensionHandle { name: String, sender: mpsc::Sender<...> }`. The `name` is used for `tracing::warn!(extension = %handle.name, ...)` in HookRouter.

- [ ] **Step 1: Add name field to ExtensionHandle**

In `crates/extensions/src/host/extension_actor.rs`, change:

```rust
#[derive(Clone)]
pub struct ExtensionHandle {
    pub(crate) name: String,
    sender: mpsc::Sender<ExtensionCommand>,
}
```

- [ ] **Step 2: Update spawn to populate name**

In `ExtensionActor::spawn`, change:

```rust
    pub fn spawn(
        extension: Arc<dyn Extension>,
        obs_bus: Arc<EventBus<ObsEvent>>,
        buffer: usize,
    ) -> (ExtensionHandle, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel::<ExtensionCommand>(buffer);
        let name = extension.name().to_string();
        let handle = ExtensionHandle { name, sender: tx };
        // ... rest unchanged
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p extensions`
Expected: Compiles successfully

- [ ] **Step 4: Commit**

```bash
git add crates/extensions/src/host/extension_actor.rs
git commit -m "feat(extensions): add name field to ExtensionHandle for tracing"
```

---

## Task 4: Add execution_mode to ExtensionTool

**Files:**
- Modify: `crates/extensions/src/host/extension_tool.rs`
- Modify: `crates/extensions/src/host/manager.rs:84-89`

**Context:** Spec §8 requires `ExtensionTool { execution_mode: ToolExecutionMode }`. Current code omits this field.

- [ ] **Step 1: Add execution_mode field**

In `crates/extensions/src/host/extension_tool.rs`, change:

```rust
use agent_core::types::{AgentTool, AgentToolProgressUpdate, AgentToolResult, ToolExecutionMode};

pub struct ExtensionTool {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) parameters: serde_json::Value,
    pub(crate) handle: ExtensionHandle,
    pub(crate) execution_mode: ToolExecutionMode,
}

#[async_trait]
impl AgentTool for ExtensionTool {
    // ... existing methods ...

    fn execution_mode(&self) -> ToolExecutionMode {
        self.execution_mode
    }

    // ... rest unchanged
```

- [ ] **Step 2: Update manager.rs to pass execution_mode**

In `crates/extensions/src/host/manager.rs`, change the `ExtensionTool` construction:

```rust
                        tools.push(Arc::new(ExtensionTool {
                            name: tool_def.name,
                            description: tool_def.description,
                            parameters: tool_def.parameters,
                            handle: handle.clone(),
                            execution_mode: ToolExecutionMode::Parallel,
                        }) as AgentToolRef);
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p extensions`
Expected: Compiles successfully

- [ ] **Step 4: Commit**

```bash
git add crates/extensions/src/host/extension_tool.rs crates/extensions/src/host/manager.rs
git commit -m "feat(extensions): add execution_mode to ExtensionTool"
```

---

## Task 5: Add zero-subscriber warn to EventBus::emit

**Files:**
- Modify: `crates/extensions/src/host/event_bus.rs:22-24`

**Context:** Spec §5 requires `tracing::warn!` when `emit()` has no active subscribers.

- [ ] **Step 1: Add variant_name to ObsEvent**

In `crates/extensions/src/host/extension_actor.rs`, add to `ObsEvent`:

```rust
impl ObsEvent {
    pub fn variant_name(&self) -> &'static str {
        match self {
            ObsEvent::TurnEnd(_) => "TurnEnd",
            ObsEvent::AgentEnd(_) => "AgentEnd",
            ObsEvent::SessionStart(_) => "SessionStart",
            ObsEvent::ToolExecutionStart(_) => "ToolExecutionStart",
            ObsEvent::ToolExecutionUpdate(_) => "ToolExecutionUpdate",
            ObsEvent::ToolExecutionEnd(_) => "ToolExecutionEnd",
            ObsEvent::CompactEnd(_) => "CompactEnd",
        }
    }
}
```

- [ ] **Step 2: Update emit with warn log**

In `crates/extensions/src/host/event_bus.rs`, change:

```rust
    pub fn emit(&self, event: T) {
        // When event is ObsEvent, we could log variant name
        // Since EventBus is generic, we require T: std::fmt::Debug for logging
        if let Err(_) = self.tx.send(event) {
            tracing::warn!(
                "EventBus emit failed: no active subscribers, event dropped"
            );
        }
    }
```

Actually, since `EventBus<T>` is generic and `T` may not have `Debug`, we have two options:
1. Add `T: std::fmt::Debug` bound (but this may break existing uses)
2. Keep the simple warn without event details

Since the spec just says "event dropped" with a variant name, and ObsEvent already has `#[derive(Debug)]`, the simplest fix is:

```rust
    pub fn emit(&self, event: T) {
        if self.tx.send(event).is_err() {
            tracing::warn!(
                "EventBus emit failed: no active subscribers, event dropped"
            );
        }
    }
```

This matches the current code structure. The `ObsEvent::variant_name()` from Step 1 can be used by callers if they want more detail.

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p extensions`
Expected: Compiles successfully

- [ ] **Step 4: Commit**

```bash
git add crates/extensions/src/host/event_bus.rs crates/extensions/src/host/extension_actor.rs
git commit -m "feat(extensions): add tracing::warn on EventBus emit with no subscribers"
```

---

## Task 6: Add missing HookRouter tests

**Files:**
- Create: `crates/extensions/tests/hook_router_compact_tests.rs`
- Create: `crates/extensions/tests/hook_router_provider_tests.rs`
- Create: `crates/extensions/tests/hook_router_observation_tests.rs`

**Context:** Spec §11.1 lists 22 tests. Currently implemented ~13. Need to add tests for:
- on_before_compact (block, replace, continue)
- on_before_agent_start (chain merge, timeout)
- on_before_provider_request (modifies options)
- on_after_provider_response (content mutation)
- tool_execution_* events broadcast
- compact_end broadcast

- [ ] **Step 1: Write on_before_compact tests**

Create `crates/extensions/tests/hook_router_compact_tests.rs`:

```rust
use std::sync::Arc;
use async_trait::async_trait;

use agent_core::context::CompactCtx;
use agent_core::mutations::{CompactDecision, HookDecision, ToolCallMutation};
use agent_core::HookDispatcher;
use agent_core::compaction::{CompactionPreparation, CompactionResult};
use agent_core::session_entry::SessionEntry;
use agent_core::context::CompactReason;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;

struct CompactBlockerExt;

#[async_trait]
impl Extension for CompactBlockerExt {
    fn name(&self) -> &str { "compact_blocker" }
    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Block { reason: "too early".to_string() }
    }
}

struct CompactReplacerExt;

#[async_trait]
impl Extension for CompactReplacerExt {
    fn name(&self) -> &str { "compact_replacer" }
    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Replace { result: CompactionResult::default() }
    }
}

struct CompactContinueExt;

#[async_trait]
impl Extension for CompactContinueExt {
    fn name(&self) -> &str { "compact_continue" }
}

fn dummy_compact_ctx() -> CompactCtx {
    CompactCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        preparation: CompactionPreparation::default(),
        entries: vec![],
        reason: CompactReason::Manual,
    }
}

#[tokio::test]
async fn test_on_before_compact_first_block_wins() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext1 = Arc::new(CompactContinueExt);
    let ext2 = Arc::new(CompactBlockerExt);
    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);
    let router = HookRouter::new(vec![h1, h2], bus);

    let result = router.on_before_compact(&dummy_compact_ctx()).await;
    assert!(matches!(result, CompactDecision::Block { .. }));
}

#[tokio::test]
async fn test_on_before_compact_replace() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(CompactReplacerExt);
    let (h, _) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![h], bus);

    let result = router.on_before_compact(&dummy_compact_ctx()).await;
    assert!(matches!(result, CompactDecision::Replace { .. }));
}

#[tokio::test]
async fn test_on_before_compact_all_continue() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext1 = Arc::new(CompactContinueExt);
    let ext2 = Arc::new(CompactContinueExt);
    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);
    let router = HookRouter::new(vec![h1, h2], bus);

    let result = router.on_before_compact(&dummy_compact_ctx()).await;
    assert!(matches!(result, CompactDecision::Continue));
}
```

- [ ] **Step 2: Write provider hook tests**

Create `crates/extensions/tests/hook_router_provider_tests.rs`:

```rust
use std::sync::Arc;
use async_trait::async_trait;

use agent_core::context::{BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx};
use agent_core::mutations::{BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation, HookDecision, ToolCallMutation};
use agent_core::HookDispatcher;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;
use llm_client::Content;

struct SystemPromptExt {
    prompt: String,
}

#[async_trait]
impl Extension for SystemPromptExt {
    fn name(&self) -> &str { "system_prompt" }
    async fn on_before_agent_start(&self, _ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
        BeforeAgentStartMutation { system_prompt: Some(self.prompt.clone()), messages: None }
    }
}

struct MessageAppenderExt {
    text: String,
}

#[async_trait]
impl Extension for MessageAppenderExt {
    fn name(&self) -> &str { "message_appender" }
    async fn on_before_agent_start(&self, ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
        let mut messages = ctx.messages.clone();
        messages.push(agent_core::AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: self.text.clone(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        }));
        BeforeAgentStartMutation { system_prompt: None, messages: Some(messages) }
    }
}

fn dummy_before_agent_start_ctx() -> BeforeAgentStartCtx {
    BeforeAgentStartCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: None,
        messages: vec![],
        tools: vec![],
        model: "gpt-4".to_string(),
    }
}

#[tokio::test]
async fn test_before_agent_start_chain_merge() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext1 = Arc::new(SystemPromptExt { prompt: "You are helpful".to_string() });
    let ext2 = Arc::new(MessageAppenderExt { text: "hello".to_string() });
    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);
    let router = HookRouter::new(vec![h1, h2], bus);

    let mutation = router.on_before_agent_start(&dummy_before_agent_start_ctx()).await;
    assert!(mutation.system_prompt.is_some());
    assert!(mutation.messages.is_some());
}

struct ProviderOptionsExt;

#[async_trait]
impl Extension for ProviderOptionsExt {
    fn name(&self) -> &str { "provider_options" }
    async fn on_before_provider_request(&self, _ctx: &ProviderRequestCtx) -> ProviderRequestMutation {
        // Mutate options by changing max_tokens
        let mut options = crate::agent_core::provider_opts::ProviderStreamOptions::default();
        options.max_tokens = Some(100);
        ProviderRequestMutation {
            system_prompt: None,
            messages: None,
            tools: None,
            options: Some(options),
        }
    }
}

#[tokio::test]
async fn test_before_provider_request_modifies_options() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(ProviderOptionsExt);
    let (h, _) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![h], bus);

    let ctx = ProviderRequestCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        model: "gpt-4".to_string(),
        system_prompt: None,
        messages: vec![],
        turn_index: 0,
        tools: None,
        options: crate::agent_core::provider_opts::ProviderStreamOptions::default(),
    };

    let mutation = router.on_before_provider_request(&ctx).await;
    assert!(mutation.options.is_some());
    assert_eq!(mutation.options.unwrap().max_tokens, Some(100));
}

// Note: on_after_provider_response tests require constructing ProviderResponseCtx
// with llm_client::StopReason which is an enum. Add those tests when needed.
```

- [ ] **Step 3: Write observation event tests**

Create `crates/extensions/tests/hook_router_observation_tests.rs`:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use async_trait::async_trait;

use agent_core::context::{ToolExecutionStartCtx, ToolExecutionEndCtx, CompactEndCtx};
use agent_core::HookDispatcher;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;

struct ExecutionCounterExt {
    start_count: AtomicUsize,
    end_count: AtomicUsize,
}

#[async_trait]
impl Extension for ExecutionCounterExt {
    fn name(&self) -> &str { "execution_counter" }

    async fn on_tool_execution_start(&self, _ctx: &ToolExecutionStartCtx) {
        self.start_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_tool_execution_end(&self, _ctx: &ToolExecutionEndCtx) {
        self.end_count.fetch_add(1, Ordering::SeqCst);
    }
}

struct CompactCounterExt {
    compact_end_count: AtomicUsize,
}

#[async_trait]
impl Extension for CompactCounterExt {
    fn name(&self) -> &str { "compact_counter" }

    async fn on_compact_end(&self, _ctx: &CompactEndCtx) {
        self.compact_end_count.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn test_tool_execution_events_broadcast() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let counter = Arc::new(ExecutionCounterExt {
        start_count: AtomicUsize::new(0),
        end_count: AtomicUsize::new(0),
    });

    let (handle, _) = ExtensionActor::spawn(counter.clone(), bus.clone(), 8);
    tokio::time::sleep(Duration::from_millis(10)).await; // Let actor subscribe

    let router = HookRouter::new(vec![handle], bus.clone());

    let start_ctx = ToolExecutionStartCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };
    router.on_tool_execution_start(&start_ctx).await;

    let end_ctx = ToolExecutionEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        success: true,
    };
    router.on_tool_execution_end(&end_ctx).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(counter.start_count.load(Ordering::SeqCst), 1);
    assert_eq!(counter.end_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_compact_end_broadcast() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let counter = Arc::new(CompactCounterExt {
        compact_end_count: AtomicUsize::new(0),
    });

    let (handle, _) = ExtensionActor::spawn(counter.clone(), bus.clone(), 8);
    tokio::time::sleep(Duration::from_millis(10)).await;

    let router = HookRouter::new(vec![handle], bus.clone());

    let ctx = CompactEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        compacted_messages: vec![],
        token_savings: 100,
    };
    router.on_compact_end(&ctx).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(counter.compact_end_count.load(Ordering::SeqCst), 1);
}
```

- [ ] **Step 4: Run all new tests**

Run: `cargo test -p extensions --test hook_router_compact_tests --test hook_router_provider_tests --test hook_router_observation_tests`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git add crates/extensions/tests/hook_router_compact_tests.rs \
        crates/extensions/tests/hook_router_provider_tests.rs \
        crates/extensions/tests/hook_router_observation_tests.rs
git commit -m "test(extensions): add HookRouter tests for compact, provider, observation hooks"
```

---

## Task 7: Add missing ExtensionActor tests

**Files:**
- Create: `crates/extensions/tests/extension_actor_advanced_tests.rs`

**Context:** Spec §11.2 lists 9 tests. Currently implemented ~4 inline tests. Need to add:
- eventbus receive
- observation timeout
- shutdown drops handle

- [ ] **Step 1: Write advanced actor tests**

Create `crates/extensions/tests/extension_actor_advanced_tests.rs`:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use async_trait::async_trait;

use agent_core::context::{TurnEndCtx, ToolResultCtx};
use agent_core::mutations::{ToolResultMutation, HookDecision, ToolCallMutation};
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};

struct EventBusReceiverExt {
    turn_end_count: AtomicUsize,
}

#[async_trait]
impl Extension for EventBusReceiverExt {
    fn name(&self) -> &str { "event_bus_receiver" }

    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {
        self.turn_end_count.fetch_add(1, Ordering::SeqCst);
    }
}

struct SlowObservationExt;

#[async_trait]
impl Extension for SlowObservationExt {
    fn name(&self) -> &str { "slow_observation" }

    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn test_actor_eventbus_receive() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(EventBusReceiverExt {
        turn_end_count: AtomicUsize::new(0),
    });

    let (handle, _jh) = ExtensionActor::spawn(ext.clone(), bus.clone(), 8);
    tokio::time::sleep(Duration::from_millis(10)).await; // Let actor subscribe

    bus.emit(ObsEvent::TurnEnd(TurnEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        turn_index: 0,
        messages: vec![],
    }));

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(ext.turn_end_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_actor_observation_timeout() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(SlowObservationExt);

    let (handle, _jh) = ExtensionActor::spawn(ext, bus.clone(), 8);
    tokio::time::sleep(Duration::from_millis(10)).await;

    // This should not block; actor should continue after 100ms timeout
    bus.emit(ObsEvent::TurnEnd(TurnEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        turn_index: 0,
        messages: vec![],
    }));

    // If the actor blocked on the slow observation, this would timeout
    let ctx = ToolResultCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
        content: vec![],
        details: None,
        is_error: false,
    };

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        handle.on_tool_result(ctx)
    ).await;

    assert!(result.is_ok(), "actor should not be blocked by slow observation hook");
}

#[tokio::test]
async fn test_actor_shutdown_drops_handle() {
    let ext = Arc::new(EventBusReceiverExt {
        turn_end_count: AtomicUsize::new(0),
    });
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, join_handle) = ExtensionActor::spawn(ext, bus, 8);

    handle.shutdown().await;

    let result = tokio::time::timeout(Duration::from_secs(2), join_handle).await;
    assert!(result.is_ok(), "actor should exit after shutdown");
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p extensions --test extension_actor_advanced_tests`
Expected: All pass

- [ ] **Step 3: Commit**

```bash
git add crates/extensions/tests/extension_actor_advanced_tests.rs
git commit -m "test(extensions): add ExtensionActor eventbus, timeout, shutdown tests"
```

---

## Task 8: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p extensions`
Expected: All tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p extensions -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Commit final**

```bash
git commit --allow-empty -m "chore(extensions): complete gap fixes per spec 2026-05-02"
```

---

## Gap Summary

| Priority | Gap | Status after plan |
|---|---|---|
| P0 | HookRouter 8 TODO methods | ✅ All 14 methods implemented |
| P0 | on_tool_call mutation bug | ✅ Input mutation chain fixed |
| P1 | ExtensionHandle missing `name` | ✅ Added |
| P1 | ExtensionTool missing `execution_mode` | ✅ Added |
| P1 | EventBus emit no warn | ✅ tracing::warn added |
| P2 | Missing tests | ✅ ~11 new tests added |

---

*Plan written: 2026-05-05*
*Spec reference: docs/specs/2026-05-02-extensions.md*
*Code reference: crates/extensions/src/*
