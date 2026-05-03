# Extensions Crate Upgrade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade the `extensions` crate from its current MVP (7 hooks, 3 ActorMessages, generic EventBus) to the full spec (14 hooks, execute_tool, 8 ActorMessages, dedicated ObservationEvent, ExtensionManager, ExtensionTool, 3 builtins, full test suite).

**Architecture:** Actor-model based extension system where each Extension runs in its own tokio task. Blocking hooks use mpsc+oneshot with 500ms timeout. Observational hooks use broadcast EventBus with 100ms timeout. Panics are isolated per-extension via tokio::spawn JoinHandles.

**Tech Stack:** Rust 2024 edition, tokio (async runtime, mpsc, broadcast, timeout), async-trait, tracing, serde_json

---

## File Map

### Existing Files to Modify
| File | Current | Target |
|---|---|---|
| `crates/extensions/src/host/extension.rs` | Extension trait with 7 hooks | Extension trait with 14 hooks + ToolCallMutation |
| `crates/extensions/src/host/extension_actor.rs` | 3 ActorMessage variants, no panic isolation | 8 ExtensionCommand variants, tokio::select!, panic isolation |
| `crates/extensions/src/host/hook_router.rs` | 6 HookDispatcher methods, no mutation chain | 14 methods, full mutation chain for all hooks |
| `crates/extensions/src/host/event_bus.rs` | Generic EventBus<T>, ObsEvent with 3 variants | Dedicated EventBus with ObservationEvent (7 variants) |
| `crates/extensions/src/host/mod.rs` | 4 module declarations | 6 module declarations |
| `crates/extensions/src/lib.rs` | Re-exports Extension, EventBus, HookRouter | Add Manager, ExtensionTool, builtins |
| `crates/extensions/src/builtins/mod.rs` | Empty stub | Re-export 3 builtins |

### New Files to Create
| File | Responsibility |
|---|---|
| `crates/extensions/src/host/manager.rs` | ExtensionManager: lifecycle, tool collection, spawn_all |
| `crates/extensions/src/host/extension_tool.rs` | ExtensionTool: wraps Extension tools into AgentTool trait |
| `crates/extensions/src/builtins/audit.rs` | AuditExtension: tracing-only observational extension |
| `crates/extensions/src/builtins/rate_limit.rs` | RateLimitExtension: sliding window rate limiting |
| `crates/extensions/src/builtins/tool_guard.rs` | ToolGuardExtension: allow/deny list tool access control |
| `crates/extensions/tests/hook_router_tests.rs` | HookRouter integration tests |
| `crates/extensions/tests/extension_actor_tests.rs` | ExtensionActor lifecycle, panic, timeout tests |
| `crates/extensions/tests/event_bus_tests.rs` | EventBus broadcast, lag, no-subscriber tests |
| `crates/extensions/tests/builtin_audit_tests.rs` | AuditExtension tests |
| `crates/extensions/tests/builtin_rate_limit_tests.rs` | RateLimitExtension tests |
| `crates/extensions/tests/builtin_tool_guard_tests.rs` | ToolGuardExtension tests |

---

## Dependencies

This plan assumes `agent-core` provides the following types. If they don't exist yet, the implementing engineer should add placeholder types in `crates/extensions/src/host/_placeholders.rs` temporarily.

**Required from agent-core:**
- Context types: `CompactCtx`, `BeforeAgentStartCtx`, `ProviderRequestCtx`, `ProviderResponseCtx`, `ToolExecutionStartCtx`, `ToolExecutionUpdateCtx`, `ToolExecutionEndCtx`, `CompactEndCtx`
- Mutation types: `CompactDecision`, `BeforeAgentStartMutation`, `ProviderRequestMutation`, `ProviderResponseMutation`
- Trait types: `AgentTool`, `AgentToolResult`, `AgentToolRef`, `AgentError`
- HookDispatcher trait with 14 methods

---

## Task 1: Define Placeholder Types (if needed)

**Files:**
- Create: `crates/extensions/src/host/_placeholders.rs` (temporary, delete after agent-core provides types)
- Modify: `crates/extensions/src/host/mod.rs`

**Context:** Before we can extend the Extension trait, agent-core must provide the new context and mutation types. If they don't exist, create placeholders here so compilation succeeds. Check first: `grep -r "CompactCtx\|BeforeAgentStartCtx" crates/agent-core/src/`

- [ ] **Step 1: Check if agent-core has the types**

Run: `grep -r "pub struct CompactCtx\|pub struct BeforeAgentStartCtx\|pub enum CompactDecision" crates/agent-core/src/`
Expected: Either file paths (types exist) or no output (need placeholders)

- [ ] **Step 2: Create placeholder types if missing**

If types are missing, create `crates/extensions/src/host/_placeholders.rs`:

```rust
use agent_core::types::AgentMessage;
use serde_json::Value;

// Context placeholders
#[derive(Debug, Clone)]
pub struct CompactCtx { pub tenant_id: String, pub session_id: String }

#[derive(Debug, Clone)]
pub struct BeforeAgentStartCtx { pub tenant_id: String, pub session_id: String, pub system_prompt: Option<String>, pub messages: Vec<AgentMessage> }

#[derive(Debug, Clone)]
pub struct ProviderRequestCtx { pub tenant_id: String, pub session_id: String, pub system_prompt: String, pub messages: Vec<AgentMessage>, pub tools: Vec<Value>, pub options: Option<Value> }

#[derive(Debug, Clone)]
pub struct ProviderResponseCtx { pub tenant_id: String, pub session_id: String, pub content: String, pub stop_reason: String }

#[derive(Debug, Clone)]
pub struct ToolExecutionStartCtx { pub tenant_id: String, pub session_id: String, pub tool_name: String, pub tool_call_id: String }

#[derive(Debug, Clone)]
pub struct ToolExecutionUpdateCtx { pub tenant_id: String, pub session_id: String, pub tool_name: String, pub tool_call_id: String, pub delta: String }

#[derive(Debug, Clone)]
pub struct ToolExecutionEndCtx { pub tenant_id: String, pub session_id: String, pub tool_name: String, pub tool_call_id: String }

#[derive(Debug, Clone)]
pub struct CompactEndCtx { pub tenant_id: String, pub session_id: String }

// Mutation placeholders
#[derive(Debug, Clone)]
pub enum CompactDecision { Continue, Block { reason: String }, Replace { result: String } }

#[derive(Debug, Clone, Default)]
pub struct BeforeAgentStartMutation { pub system_prompt: Option<String>, pub messages: Option<Vec<AgentMessage>> }

#[derive(Debug, Clone, Default)]
pub struct ProviderRequestMutation { pub system_prompt: Option<String>, pub messages: Option<Vec<AgentMessage>>, pub tools: Option<Vec<Value>>, pub options: Option<Value> }

#[derive(Debug, Clone, Default)]
pub struct ProviderResponseMutation { pub content: Option<String>, pub stop_reason: Option<String> }
```

- [ ] **Step 3: Register placeholder module**

In `crates/extensions/src/host/mod.rs`, add at the top:
```rust
#[cfg(not(feature = "agent-core-full"))]
pub mod _placeholders;
```

- [ ] **Step 4: Verify compilation**

Run: `cargo build -p extensions`
Expected: Compiles successfully (placeholders available)

- [ ] **Step 5: Commit**

```bash
git add crates/extensions/src/host/_placeholders.rs crates/extensions/src/host/mod.rs
git commit -m "feat(extensions): add placeholder types for agent-core dependencies"
```

---

## Task 2: Extend Extension Trait with ToolCallMutation

**Files:**
- Modify: `crates/extensions/src/host/extension.rs`

**Context:** The Extension trait needs a new return type for `on_tool_call` that supports input mutation. We define `ToolCallMutation` locally in this crate (it's specific to extensions).

- [ ] **Step 1: Add ToolCallMutation struct**

At the top of `crates/extensions/src/host/extension.rs`, after the imports, add:

```rust
/// Mutation returned by blocking hooks for tool calls.
/// Unlike other blocking hooks, on_tool_call supports input mutation
/// to enable parameter sanitization, injection, and transformation.
#[derive(Debug, Clone, Default)]
pub struct ToolCallMutation {
    /// Replaced tool input parameters. When Some, replaces the original
    /// `ctx.input` for subsequent handlers and tool execution.
    pub input: Option<serde_json::Value>,
}
```

- [ ] **Step 2: Change on_tool_call signature**

Change line 16-18 from:
```rust
async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> HookDecision {
    HookDecision::Continue
}
```

To:
```rust
async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
    (HookDecision::Continue, ToolCallMutation::default())
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p extensions`
Expected: Compilation may fail due to existing implementations returning HookDecision — that's expected and fixed in next tasks

- [ ] **Step 4: Commit**

```bash
git add crates/extensions/src/host/extension.rs
git commit -m "feat(extensions): add ToolCallMutation and update on_tool_call signature"
```

---

## Task 3: Add New Hook Methods to Extension Trait

**Files:**
- Modify: `crates/extensions/src/host/extension.rs`
- Modify: `crates/extensions/src/host/extension_actor.rs` (test implementations)
- Modify: `crates/extensions/src/host/hook_router.rs` (test implementations)

**Context:** Add the remaining 7 hooks + execute_tool to the Extension trait. These require agent-core types (either real or placeholders from Task 1).

- [ ] **Step 1: Add imports for new types**

At the top of `crates/extensions/src/host/extension.rs`, replace the existing imports with:

```rust
use async_trait::async_trait;

use agent_core::context::{
    AgentEndCtx, ContextCtx, SessionCtx, ToolCallCtx, ToolResultCtx, TurnEndCtx,
    BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx,
    ToolExecutionStartCtx, ToolExecutionUpdateCtx, ToolExecutionEndCtx,
    CompactCtx, CompactEndCtx,
};
use agent_core::mutations::{
    ContextMutation, HookDecision, ToolResultMutation,
    CompactDecision, BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation,
};
use agent_core::types::{AgentToolResult, AgentError};
use llm_client::ToolDef;
```

If using placeholders, adjust the import path (e.g., `use crate::host::_placeholders::{...}`).

- [ ] **Step 2: Add new hook methods to trait**

After `on_context` (line 27) and before `on_turn_end`, add:

```rust
    // ═══ Blocking hooks — first-block-wins ═══

    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Continue
    }

    // ═══ Chaining hooks — chain merge ═══

    async fn on_before_agent_start(&self, _ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
        BeforeAgentStartMutation::default()
    }

    async fn on_before_provider_request(&self, _ctx: &ProviderRequestCtx) -> ProviderRequestMutation {
        ProviderRequestMutation::default()
    }

    async fn on_after_provider_response(&self, _ctx: &ProviderResponseCtx) -> ProviderResponseMutation {
        ProviderResponseMutation::default()
    }

    // ═══ Tool execution — runs to completion, no timeout ═══

    async fn execute_tool(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
        Err(AgentError::ToolExecutionFailed(
            "tool defined but not executable by this extension".into(),
        ))
    }
```

After `on_session_start` (end of trait), add:

```rust
    async fn on_tool_execution_start(&self, _ctx: &ToolExecutionStartCtx) {}
    async fn on_tool_execution_update(&self, _ctx: &ToolExecutionUpdateCtx) {}
    async fn on_tool_execution_end(&self, _ctx: &ToolExecutionEndCtx) {}
    async fn on_compact_end(&self, _ctx: &CompactEndCtx) {}
```

- [ ] **Step 3: Update test implementations in extension_actor.rs**

In `crates/extensions/src/host/extension_actor.rs`, the test struct `TestExtension` implements `on_tool_call`. Update it to return `(HookDecision, ToolCallMutation)`:

```rust
async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
    if ctx.tool_name == "blocked_tool" {
        (HookDecision::Block { reason: "blocked".to_string() }, ToolCallMutation::default())
    } else {
        (HookDecision::Continue, ToolCallMutation::default())
    }
}
```

- [ ] **Step 4: Update test implementations in hook_router.rs**

In `crates/extensions/src/host/hook_router.rs`, update `BlockExt`:

```rust
async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
    (HookDecision::Block { reason: "no".to_string() }, ToolCallMutation::default())
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo build -p extensions`
Expected: Compiles successfully

- [ ] **Step 6: Commit**

```bash
git add crates/extensions/src/host/extension.rs crates/extensions/src/host/extension_actor.rs crates/extensions/src/host/hook_router.rs
git commit -m "feat(extensions): add 7 new hooks and execute_tool to Extension trait"
```

---

## Task 4: Create ObservationEvent Enum

**Files:**
- Modify: `crates/extensions/src/host/event_bus.rs`
- Modify: `crates/extensions/src/lib.rs`

**Context:** Replace the generic `EventBus<T>` with a dedicated event type. This aligns with the spec's §5.

- [ ] **Step 1: Define ObservationEvent**

Replace the entire contents of `crates/extensions/src/host/event_bus.rs` with:

```rust
use tokio::sync::broadcast;
use std::time::Duration;

use agent_core::context::{
    TurnEndCtx, AgentEndCtx, SessionCtx,
    ToolExecutionStartCtx, ToolExecutionUpdateCtx, ToolExecutionEndCtx,
    CompactEndCtx,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(100);

/// Observational events delivered via broadcast to all extension actors.
#[derive(Debug, Clone)]
pub enum ObservationEvent {
    TurnEnd(TurnEndCtx),
    AgentEnd(AgentEndCtx),
    SessionStart(SessionCtx),
    ToolExecutionStart(ToolExecutionStartCtx),
    ToolExecutionUpdate(ToolExecutionUpdateCtx),
    ToolExecutionEnd(ToolExecutionEndCtx),
    CompactEnd(CompactEndCtx),
}

impl ObservationEvent {
    pub fn variant_name(&self) -> &'static str {
        match self {
            ObservationEvent::TurnEnd(_) => "TurnEnd",
            ObservationEvent::AgentEnd(_) => "AgentEnd",
            ObservationEvent::SessionStart(_) => "SessionStart",
            ObservationEvent::ToolExecutionStart(_) => "ToolExecutionStart",
            ObservationEvent::ToolExecutionUpdate(_) => "ToolExecutionUpdate",
            ObservationEvent::ToolExecutionEnd(_) => "ToolExecutionEnd",
            ObservationEvent::CompactEnd(_) => "CompactEnd",
        }
    }
}

pub struct EventBus {
    tx: broadcast::Sender<ObservationEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ObservationEvent> {
        self.tx.subscribe()
    }

    pub fn emit(&self, event: ObservationEvent) {
        if self.tx.send(event).is_err() {
            tracing::warn!(
                event = %self.last_variant_name(),
                "EventBus emit failed: no active subscribers, event dropped"
            );
        }
    }

    fn last_variant_name(&self) -> &'static str {
        "unknown"
    }
}

pub fn spawn_listener<F, Fut>(
    mut rx: broadcast::Receiver<ObservationEvent>,
    handler: F,
) -> tokio::task::JoinHandle<()>
where
    F: Fn(ObservationEvent) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let fut = handler(event);
                    let _ = tokio::time::timeout(DEFAULT_TIMEOUT, fut).await;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("EventBus listener lagged by {} messages", n);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    })
}
```

**Note:** The `last_variant_name()` hack is temporary. In the actual `emit()`, we should pass the variant name before sending. Let's fix that:

In `emit()`, change to:
```rust
    pub fn emit(&self, event: ObservationEvent) {
        let variant = event.variant_name();
        if self.tx.send(event).is_err() {
            tracing::warn!(
                event = %variant,
                "EventBus emit failed: no active subscribers, event dropped"
            );
        }
    }
```

And remove the `last_variant_name` method.

- [ ] **Step 2: Update lib.rs re-export**

In `crates/extensions/src/lib.rs`, ensure it exports `ObservationEvent`:
```rust
pub use host::event_bus::{EventBus, ObservationEvent};
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p extensions`
Expected: May fail due to ObsEvent references in hook_router.rs and extension_actor.rs — fix in Task 5

- [ ] **Step 4: Commit**

```bash
git add crates/extensions/src/host/event_bus.rs crates/extensions/src/lib.rs
git commit -m "feat(extensions): replace generic EventBus with dedicated ObservationEvent"
```

---

## Task 5: Refactor ExtensionActor

**Files:**
- Modify: `crates/extensions/src/host/extension_actor.rs`

**Context:** Complete rewrite of the actor to support 8 commands, tokio::select!, panic isolation, and inline observation handling (replacing spawn_listener).

- [ ] **Step 1: Define ExtensionCommand enum**

Replace the `ActorMessage` enum with:

```rust
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use agent_core::context::{
    AgentEndCtx, ContextCtx, SessionCtx, ToolCallCtx, ToolResultCtx, TurnEndCtx,
    BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx,
    ToolExecutionStartCtx, ToolExecutionUpdateCtx, ToolExecutionEndCtx,
    CompactCtx, CompactEndCtx,
};
use agent_core::mutations::{
    ContextMutation, HookDecision, ToolResultMutation,
    CompactDecision, BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation,
};
use agent_core::types::{AgentToolResult, AgentError};

use super::event_bus::{EventBus, ObservationEvent};
use super::extension::{Extension, ToolCallMutation};

const BLOCKING_TIMEOUT: Duration = Duration::from_millis(500);
const OBSERVATION_TIMEOUT: Duration = Duration::from_millis(100);

enum ExtensionCommand {
    OnToolCall {
        ctx: ToolCallCtx,
        reply: oneshot::Sender<(HookDecision, ToolCallMutation)>,
    },
    OnBeforeCompact {
        ctx: CompactCtx,
        reply: oneshot::Sender<CompactDecision>,
    },
    OnToolResult {
        ctx: ToolResultCtx,
        reply: oneshot::Sender<ToolResultMutation>,
    },
    OnContext {
        ctx: ContextCtx,
        reply: oneshot::Sender<ContextMutation>,
    },
    OnBeforeAgentStart {
        ctx: BeforeAgentStartCtx,
        reply: oneshot::Sender<BeforeAgentStartMutation>,
    },
    OnBeforeProviderRequest {
        ctx: ProviderRequestCtx,
        reply: oneshot::Sender<ProviderRequestMutation>,
    },
    OnAfterProviderResponse {
        ctx: ProviderResponseCtx,
        reply: oneshot::Sender<ProviderResponseMutation>,
    },
    OnExecuteTool {
        tool_call_id: String,
        params: serde_json::Value,
        reply: oneshot::Sender<Result<AgentToolResult, AgentError>>,
    },
    Shutdown,
}
```

- [ ] **Step 2: Redefine ExtensionHandle with ask pattern**

```rust
#[derive(Clone)]
pub struct ExtensionHandle {
    name: String,
    sender: mpsc::Sender<ExtensionCommand>,
}

#[derive(Debug)]
pub enum AskError {
    Timeout,
    ActorGone,
}

impl ExtensionHandle {
    async fn ask<T>(
        &self,
        command_builder: impl FnOnce(oneshot::Sender<T>) -> ExtensionCommand,
        timeout: Duration,
    ) -> Result<T, AskError> {
        let (tx, rx) = oneshot::channel();
        let cmd = command_builder(tx);
        self.sender.send(cmd).await.map_err(|_| AskError::ActorGone)?;
        tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| AskError::Timeout)?
            .map_err(|_| AskError::ActorGone)
    }

    pub async fn on_tool_call(&self, ctx: ToolCallCtx) -> Result<(HookDecision, ToolCallMutation), AskError> {
        self.ask(|reply| ExtensionCommand::OnToolCall { ctx, reply }, BLOCKING_TIMEOUT).await
    }

    pub async fn on_before_compact(&self, ctx: CompactCtx) -> Result<CompactDecision, AskError> {
        self.ask(|reply| ExtensionCommand::OnBeforeCompact { ctx, reply }, BLOCKING_TIMEOUT).await
    }

    pub async fn on_tool_result(&self, ctx: ToolResultCtx) -> Result<ToolResultMutation, AskError> {
        self.ask(|reply| ExtensionCommand::OnToolResult { ctx, reply }, BLOCKING_TIMEOUT).await
    }

    pub async fn on_context(&self, ctx: ContextCtx) -> Result<ContextMutation, AskError> {
        self.ask(|reply| ExtensionCommand::OnContext { ctx, reply }, BLOCKING_TIMEOUT).await
    }

    pub async fn on_before_agent_start(&self, ctx: BeforeAgentStartCtx) -> Result<BeforeAgentStartMutation, AskError> {
        self.ask(|reply| ExtensionCommand::OnBeforeAgentStart { ctx, reply }, BLOCKING_TIMEOUT).await
    }

    pub async fn on_before_provider_request(&self, ctx: ProviderRequestCtx) -> Result<ProviderRequestMutation, AskError> {
        self.ask(|reply| ExtensionCommand::OnBeforeProviderRequest { ctx, reply }, BLOCKING_TIMEOUT).await
    }

    pub async fn on_after_provider_response(&self, ctx: ProviderResponseCtx) -> Result<ProviderResponseMutation, AskError> {
        self.ask(|reply| ExtensionCommand::OnAfterProviderResponse { ctx, reply }, BLOCKING_TIMEOUT).await
    }

    pub async fn execute_tool(&self, tool_call_id: String, params: serde_json::Value) -> Result<AgentToolResult, AgentError> {
        let (tx, rx) = oneshot::channel();
        let cmd = ExtensionCommand::OnExecuteTool { tool_call_id, params, reply: tx };
        self.sender.send(cmd).await.map_err(|_| {
            AgentError::ToolExecutionFailed("extension actor terminated".into())
        })?;
        rx.await.map_err(|_| {
            AgentError::ToolExecutionFailed("extension actor terminated during execution".into())
        })?
    }
}
```

- [ ] **Step 3: Rewrite ExtensionActor with select! and panic isolation**

```rust
pub struct ExtensionActor;

impl ExtensionActor {
    pub fn spawn(
        extension: Arc<dyn Extension>,
        event_bus: Arc<EventBus>,
        buffer: usize,
        tenant_id: String,
        session_id: String,
    ) -> (ExtensionHandle, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel::<ExtensionCommand>(buffer);
        let name = extension.name().to_string();
        let handle = ExtensionHandle { name: name.clone(), sender: tx };

        let join_handle = tokio::spawn(async move {
            run_actor(extension, rx, event_bus, tenant_id, session_id).await;
        });

        (handle, join_handle)
    }
}

async fn run_actor(
    extension: Arc<dyn Extension>,
    mut mailbox: mpsc::Receiver<ExtensionCommand>,
    event_bus: Arc<EventBus>,
    tenant_id: String,
    session_id: String,
) {
    let mut event_bus_rx = event_bus.subscribe();

    loop {
        tokio::select! {
            cmd = mailbox.recv() => {
                match cmd {
                    Some(ExtensionCommand::OnToolCall { ctx, reply }) => {
                        handle_blocking_hook(&*extension, ctx, reply, "on_tool_call", |ext, ctx| {
                            Box::pin(async move { ext.on_tool_call(&ctx).await })
                        }).await;
                    }
                    Some(ExtensionCommand::OnBeforeCompact { ctx, reply }) => {
                        handle_blocking_hook(&*extension, ctx, reply, "on_before_compact", |ext, ctx| {
                            Box::pin(async move { ext.on_before_compact(&ctx).await })
                        }).await;
                    }
                    Some(ExtensionCommand::OnToolResult { ctx, reply }) => {
                        handle_chain_hook(&*extension, ctx, reply, "on_tool_result", |ext, ctx| {
                            Box::pin(async move { ext.on_tool_result(&ctx).await })
                        }).await;
                    }
                    Some(ExtensionCommand::OnContext { ctx, reply }) => {
                        handle_chain_hook(&*extension, ctx, reply, "on_context", |ext, ctx| {
                            Box::pin(async move { ext.on_context(&ctx).await })
                        }).await;
                    }
                    Some(ExtensionCommand::OnBeforeAgentStart { ctx, reply }) => {
                        handle_chain_hook(&*extension, ctx, reply, "on_before_agent_start", |ext, ctx| {
                            Box::pin(async move { ext.on_before_agent_start(&ctx).await })
                        }).await;
                    }
                    Some(ExtensionCommand::OnBeforeProviderRequest { ctx, reply }) => {
                        handle_chain_hook(&*extension, ctx, reply, "on_before_provider_request", |ext, ctx| {
                            Box::pin(async move { ext.on_before_provider_request(&ctx).await })
                        }).await;
                    }
                    Some(ExtensionCommand::OnAfterProviderResponse { ctx, reply }) => {
                        handle_chain_hook(&*extension, ctx, reply, "on_after_provider_response", |ext, ctx| {
                            Box::pin(async move { ext.on_after_provider_response(&ctx).await })
                        }).await;
                    }
                    Some(ExtensionCommand::OnExecuteTool { tool_call_id, params, reply }) => {
                        let ext = extension.clone();
                        tokio::spawn(async move {
                            let result = ext.execute_tool(&tool_call_id, params).await;
                            let _ = reply.send(result);
                        });
                    }
                    Some(ExtensionCommand::Shutdown) | None => break,
                }
            }
            Ok(event) = event_bus_rx.recv() => {
                handle_observation(&*extension, event, &tenant_id, &session_id).await;
            }
        }
    }
}

async fn handle_blocking_hook<Ctx, Output, F>(
    extension: &dyn Extension,
    ctx: Ctx,
    reply: oneshot::Sender<Output>,
    hook_name: &'static str,
    f: F,
)
where
    Ctx: Send + 'static + Clone,
    Output: Default + Send + 'static,
    F: FnOnce(&dyn Extension, Ctx) -> std::pin::Pin<Box<dyn std::future::Future<Output = Output> + Send>> + Send + 'static,
{
    let ext = Arc::new(extension);
    let handle = tokio::spawn(async move {
        let span = tracing::info_span!("extension_hook", hook = %hook_name);
        let _enter = span.enter();
        f(&**ext, ctx).await
    });

    match handle.await {
        Ok(result) => { let _ = reply.send(result); }
        Err(join_err) => {
            tracing::error!(hook = %hook_name, "extension panicked in blocking hook, returning default");
            if let Ok(panic_msg) = join_err.try_into_panic()
                .and_then(|p| p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())))
            {
                tracing::error!(panic_msg = %panic_msg, "panic detail");
            }
            let _ = reply.send(Output::default());
        }
    }
}

async fn handle_chain_hook<Ctx, Output, F>(
    extension: &dyn Extension,
    ctx: Ctx,
    reply: oneshot::Sender<Output>,
    hook_name: &'static str,
    f: F,
)
where
    Ctx: Send + 'static + Clone,
    Output: Default + Send + 'static,
    F: FnOnce(&dyn Extension, Ctx) -> std::pin::Pin<Box<dyn std::future::Future<Output = Output> + Send>> + Send + 'static,
{
    handle_blocking_hook(extension, ctx, reply, hook_name, f).await
}

async fn handle_observation(
    extension: &dyn Extension,
    event: ObservationEvent,
    tenant_id: &str,
    session_id: &str,
) {
    let ext = Arc::new(extension);
    let tenant_id = tenant_id.to_string();
    let session_id = session_id.to_string();

    tokio::spawn(async move {
        let result = tokio::time::timeout(
            OBSERVATION_TIMEOUT,
            async {
                match event {
                    ObservationEvent::TurnEnd(ctx) => ext.on_turn_end(&ctx).await,
                    ObservationEvent::AgentEnd(ctx) => ext.on_agent_end(&ctx).await,
                    ObservationEvent::SessionStart(ctx) => ext.on_session_start(&ctx).await,
                    ObservationEvent::ToolExecutionStart(ctx) => ext.on_tool_execution_start(&ctx).await,
                    ObservationEvent::ToolExecutionUpdate(ctx) => ext.on_tool_execution_update(&ctx).await,
                    ObservationEvent::ToolExecutionEnd(ctx) => ext.on_tool_execution_end(&ctx).await,
                    ObservationEvent::CompactEnd(ctx) => ext.on_compact_end(&ctx).await,
                }
            },
        ).await;

        if result.is_err() {
            tracing::warn!(
                "observation hook timed out after 100ms, silently dropped"
            );
        }
    });
}
```

**Note:** The `Arc::new(extension)` pattern for `&dyn Extension` won't work directly because `Extension` is not `Sized`. We need to clone the `Arc<dyn Extension>` instead. Adjust the handle_* functions to take `Arc<dyn Extension>`:

```rust
async fn handle_blocking_hook<Ctx, Output, F>(
    extension: Arc<dyn Extension>,
    ctx: Ctx,
    reply: oneshot::Sender<Output>,
    hook_name: &'static str,
    f: F,
)
where
    Ctx: Send + 'static + Clone,
    Output: Default + Send + 'static,
    F: FnOnce(Arc<dyn Extension>, Ctx) -> std::pin::Pin<Box<dyn std::future::Future<Output = Output> + Send>> + Send + 'static,
{
    let handle = tokio::spawn(async move {
        let span = tracing::info_span!("extension_hook", hook = %hook_name);
        let _enter = span.enter();
        f(extension, ctx).await
    });
    // ... rest same
}
```

And update all call sites to pass `extension.clone()` instead of `&*extension`.

- [ ] **Step 4: Update tests**

The existing tests in `extension_actor.rs` use `EventBus::<ObsEvent>::new(16)`. Update to:
```rust
let bus = Arc::new(EventBus::new(16));
```

And update spawn calls to include tenant_id and session_id:
```rust
let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8, "t1".to_string(), "s1".to_string());
```

- [ ] **Step 5: Verify compilation**

Run: `cargo build -p extensions`
Expected: Compiles successfully

- [ ] **Step 6: Run existing tests**

Run: `cargo test -p extensions -- extension_actor`
Expected: All 3 existing tests pass

- [ ] **Step 7: Commit**

```bash
git add crates/extensions/src/host/extension_actor.rs
git commit -m "feat(extensions): refactor ExtensionActor with 8 commands, select!, panic isolation"
```

---

## Task 6: Upgrade HookRouter

**Files:**
- Modify: `crates/extensions/src/host/hook_router.rs`

**Context:** Implement all 14 HookDispatcher methods with proper mutation chains.

- [ ] **Step 1: Update imports and struct**

Replace the top of the file with:

```rust
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use agent_core::context::{
    AgentEndCtx, ContextCtx, SessionCtx, ToolCallCtx, ToolResultCtx, TurnEndCtx,
    BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx,
    ToolExecutionStartCtx, ToolExecutionUpdateCtx, ToolExecutionEndCtx,
    CompactCtx, CompactEndCtx,
};
use agent_core::mutations::{
    ContextMutation, HookDecision, ToolResultMutation,
    CompactDecision, BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation,
};
use agent_core::HookDispatcher;

use super::event_bus::{EventBus, ObservationEvent};
use super::extension_actor::{ExtensionHandle, AskError};
use super::extension::ToolCallMutation;

pub struct HookRouter {
    handles: Vec<ExtensionHandle>,
    event_bus: Arc<EventBus>,
    blocking_timeout: Duration,
}

impl HookRouter {
    pub fn new(handles: Vec<ExtensionHandle>, event_bus: Arc<EventBus>) -> Self {
        Self {
            handles,
            event_bus,
            blocking_timeout: Duration::from_millis(500),
        }
    }
}
```

- [ ] **Step 2: Implement on_tool_call with mutation chain**

```rust
#[async_trait]
impl HookDispatcher for HookRouter {
    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        let mut current_ctx = ctx.clone();
        for handle in &self.handles {
            let (decision, mutation) = match handle.on_tool_call(current_ctx.clone()).await {
                Ok(result) => result,
                Err(AskError::Timeout) => {
                    tracing::warn!(extension = %handle.name, "on_tool_call timeout, treating as Continue");
                    (HookDecision::Continue, ToolCallMutation::default())
                }
                Err(AskError::ActorGone) => {
                    tracing::error!(extension = %handle.name, "ExtensionActor terminated unexpectedly");
                    (HookDecision::Continue, ToolCallMutation::default())
                }
            };

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

- [ ] **Step 3: Implement on_before_compact**

```rust
    async fn on_before_compact(&self, ctx: &CompactCtx) -> CompactDecision {
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

- [ ] **Step 4: Implement on_tool_result with chain merge**

```rust
    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
        let mut accumulated = ToolResultMutation::default();
        let mut current_ctx = ctx.clone();

        for handle in &self.handles {
            match handle.on_tool_result(current_ctx.clone()).await {
                Ok(mutation) => {
                    if let Some(ref content) = mutation.content {
                        current_ctx.content = content.clone();
                        accumulated.content = Some(content.clone());
                    }
                    if let Some(ref details) = mutation.details {
                        current_ctx.details = Some(details.clone());
                        accumulated.details = Some(details.clone());
                    }
                    if let Some(is_error) = mutation.is_error {
                        current_ctx.is_error = is_error;
                        accumulated.is_error = Some(is_error);
                    }
                }
                Err(AskError::Timeout) => {
                    tracing::warn!(extension = %handle.name, "on_tool_result timeout, skipping");
                    continue;
                }
                Err(AskError::ActorGone) => continue,
            }
        }
        accumulated
    }
```

- [ ] **Step 5: Implement on_context**

```rust
    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        let mut accumulated = ContextMutation::default();
        let mut current_messages = ctx.messages.clone();

        for handle in &self.handles {
            let ctx = ContextCtx {
                tenant_id: ctx.tenant_id.clone(),
                session_id: ctx.session_id.clone(),
                messages: current_messages.clone(),
            };
            match handle.on_context(ctx).await {
                Ok(mutation) => {
                    if let Some(ref msgs) = mutation.messages {
                        current_messages = msgs.clone();
                        accumulated.messages = Some(msgs.clone());
                    }
                }
                Err(AskError::Timeout) => {
                    tracing::warn!(extension = %handle.name, "on_context timeout, skipping");
                    continue;
                }
                Err(AskError::ActorGone) => continue,
            }
        }
        accumulated
    }
```

- [ ] **Step 6: Implement remaining chain hooks**

```rust
    async fn on_before_agent_start(&self, ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
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

    async fn on_before_provider_request(&self, ctx: &ProviderRequestCtx) -> ProviderRequestMutation {
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
                        current_ctx.options = Some(options.clone());
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

    async fn on_after_provider_response(&self, ctx: &ProviderResponseCtx) -> ProviderResponseMutation {
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

- [ ] **Step 7: Implement observational hooks**

```rust
    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        self.event_bus.emit(ObservationEvent::TurnEnd(ctx.clone()));
    }

    async fn on_agent_end(&self, ctx: &AgentEndCtx) {
        self.event_bus.emit(ObservationEvent::AgentEnd(ctx.clone()));
    }

    async fn on_session_start(&self, ctx: &SessionCtx) {
        self.event_bus.emit(ObservationEvent::SessionStart(ctx.clone()));
    }

    async fn on_tool_execution_start(&self, ctx: &ToolExecutionStartCtx) {
        self.event_bus.emit(ObservationEvent::ToolExecutionStart(ctx.clone()));
    }

    async fn on_tool_execution_update(&self, ctx: &ToolExecutionUpdateCtx) {
        self.event_bus.emit(ObservationEvent::ToolExecutionUpdate(ctx.clone()));
    }

    async fn on_tool_execution_end(&self, ctx: &ToolExecutionEndCtx) {
        self.event_bus.emit(ObservationEvent::ToolExecutionEnd(ctx.clone()));
    }

    async fn on_compact_end(&self, ctx: &CompactEndCtx) {
        self.event_bus.emit(ObservationEvent::CompactEnd(ctx.clone()));
    }
}
```

- [ ] **Step 8: Update tests**

The existing tests use `EventBus::<ObsEvent>::new(16)` and `ExtensionActor::spawn(...)` with old signatures. Update them:

```rust
let bus = Arc::new(EventBus::new(16));
// ...
let (h1, _jh1) = ExtensionActor::spawn(ext1, bus.clone(), 8, "t1".to_string(), "s1".to_string());
```

Also update `BlockExt::on_tool_call` to return `(HookDecision, ToolCallMutation)`.

- [ ] **Step 9: Verify compilation and tests**

Run: `cargo build -p extensions`
Expected: Compiles successfully

Run: `cargo test -p extensions -- hook_router`
Expected: Tests pass

- [ ] **Step 10: Commit**

```bash
git add crates/extensions/src/host/hook_router.rs
git commit -m "feat(extensions): upgrade HookRouter with 14 methods and full mutation chains"
```

---

## Task 7: Implement ExtensionManager

**Files:**
- Create: `crates/extensions/src/host/manager.rs`
- Modify: `crates/extensions/src/host/mod.rs`

**Context:** Manager handles extension lifecycle, tool collection, and actor spawning.

- [ ] **Step 1: Create manager.rs**

```rust
use std::sync::Arc;
use std::collections::HashSet;

use llm_client::ToolDef;

use super::event_bus::EventBus;
use super::extension::Extension;
use super::extension_actor::{ExtensionActor, ExtensionHandle};
use super::hook_router::HookRouter;

pub struct ExtensionManager {
    extensions: Vec<Arc<dyn Extension>>,
    event_bus_capacity: usize,
}

impl ExtensionManager {
    pub fn new(extensions: Vec<Arc<dyn Extension>>) -> Self {
        Self {
            extensions,
            event_bus_capacity: 128,
        }
    }

    pub fn collect_tools(&self) -> Vec<ToolDef> {
        let mut seen = HashSet::new();
        let mut tools = Vec::new();
        for ext in &self.extensions {
            for tool in ext.tools() {
                if seen.insert(tool.name.clone()) {
                    tools.push(tool);
                }
            }
        }
        tools
    }

    pub fn spawn_all(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> (HookRouter, Vec<ExtensionHandle>, Vec<tokio::task::JoinHandle<()>>) {
        let event_bus = Arc::new(EventBus::new(self.event_bus_capacity));
        let mut handles = Vec::new();
        let mut ext_handles = Vec::new();
        let mut join_handles = Vec::new();

        for ext in &self.extensions {
            let (tx, rx) = tokio::sync::mpsc::channel(32);
            // Note: ExtensionActor::spawn signature changed in Task 5
            let (handle, join_handle) = ExtensionActor::spawn(
                ext.clone(),
                event_bus.clone(),
                32,
                tenant_id.to_string(),
                session_id.to_string(),
            );

            join_handles.push(join_handle);
            ext_handles.push(handle);
        }

        let hook_router = HookRouter::new(ext_handles.clone(), event_bus);

        (hook_router, ext_handles, join_handles)
    }
}
```

Wait, there's a problem. `ExtensionActor::spawn` in Task 5 returns `(ExtensionHandle, JoinHandle)` but also creates its own mpsc channel internally. The manager shouldn't create another channel. Let me check Task 5's spawn signature:

```rust
pub fn spawn(
    extension: Arc<dyn Extension>,
    event_bus: Arc<EventBus>,
    buffer: usize,
    tenant_id: String,
    session_id: String,
) -> (ExtensionHandle, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<ExtensionCommand>(buffer);
    // ...
}
```

OK, so `spawn` creates the channel. Manager just calls `spawn` for each extension. The manager.rs code above is correct (without the extra `mpsc::channel(32)` line — remove that).

Corrected:
```rust
        for ext in &self.extensions {
            let (handle, join_handle) = ExtensionActor::spawn(
                ext.clone(),
                event_bus.clone(),
                32,
                tenant_id.to_string(),
                session_id.to_string(),
            );

            join_handles.push(join_handle);
            ext_handles.push(handle);
        }
```

- [ ] **Step 2: Register module**

In `crates/extensions/src/host/mod.rs`, add:
```rust
pub mod manager;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p extensions`
Expected: Compiles successfully

- [ ] **Step 4: Commit**

```bash
git add crates/extensions/src/host/manager.rs crates/extensions/src/host/mod.rs
git commit -m "feat(extensions): add ExtensionManager for lifecycle and tool collection"
```

---

## Task 8: Implement ExtensionTool

**Files:**
- Create: `crates/extensions/src/host/extension_tool.rs`
- Modify: `crates/extensions/src/host/mod.rs`

**Context:** Wraps Extension tools into AgentTool trait objects.

- [ ] **Step 1: Create extension_tool.rs**

```rust
use std::sync::Arc;

use agent_core::types::{AgentTool, AgentToolResult, AgentToolRef};
use agent_core::error::AgentError;

use super::extension_actor::ExtensionHandle;

struct ExtensionTool {
    name: String,
    description: String,
    parameters: serde_json::Value,
    handle: ExtensionHandle,
}

#[async_trait::async_trait]
impl AgentTool for ExtensionTool {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { &self.description }
    fn parameters(&self) -> serde_json::Value { self.parameters.clone() }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        _on_progress: Option<&(dyn Fn(agent_core::types::AgentToolProgressUpdate) + Send + Sync)>,
    ) -> Result<AgentToolResult, AgentError> {
        self.handle.execute_tool(tool_call_id.to_string(), params).await
    }
}

pub fn collect_extension_tools(
    handles: &[ExtensionHandle],
    extensions: &[Arc<dyn super::extension::Extension>],
) -> Vec<AgentToolRef> {
    use llm_client::ToolDef;
    use std::collections::HashSet;

    let mut seen = HashSet::new();
    let mut tools = Vec::new();

    for (i, ext) in extensions.iter().enumerate() {
        if let Some(handle) = handles.get(i) {
            for tool_def in ext.tools() {
                if seen.insert(tool_def.name.clone()) {
                    tools.push(Arc::new(ExtensionTool {
                        name: tool_def.name,
                        description: tool_def.description,
                        parameters: tool_def.parameters,
                        handle: handle.clone(),
                    }) as AgentToolRef);
                }
            }
        }
    }
    tools
}
```

- [ ] **Step 2: Register module**

In `crates/extensions/src/host/mod.rs`, add:
```rust
pub mod extension_tool;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p extensions`
Expected: Compiles successfully

- [ ] **Step 4: Commit**

```bash
git add crates/extensions/src/host/extension_tool.rs crates/extensions/src/host/mod.rs
git commit -m "feat(extensions): add ExtensionTool wrapper for AgentTool trait"
```

---

## Task 9: Implement Built-in Extensions

**Files:**
- Create: `crates/extensions/src/builtins/audit.rs`
- Create: `crates/extensions/src/builtins/rate_limit.rs`
- Create: `crates/extensions/src/builtins/tool_guard.rs`
- Modify: `crates/extensions/src/builtins/mod.rs`

**Context:** Three simple built-in extensions.

- [ ] **Step 1: Create audit.rs**

```rust
use async_trait::async_trait;

use crate::host::extension::{Extension, ToolCallMutation};
use agent_core::context::{ToolCallCtx, ToolResultCtx, TurnEndCtx};
use agent_core::mutations::{HookDecision, ToolResultMutation};

pub struct AuditExtension;

#[async_trait]
impl Extension for AuditExtension {
    fn name(&self) -> &str { "audit" }

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        tracing::info!(
            target: "pandaria.audit",
            tool_name = %ctx.tool_name,
            tool_call_id = %ctx.tool_call_id,
            action = "tool_call_start"
        );
        (HookDecision::Continue, ToolCallMutation::default())
    }

    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
        tracing::info!(
            target: "pandaria.audit",
            tool_name = %ctx.tool_name,
            tool_call_id = %ctx.tool_call_id,
            is_error = ctx.is_error,
            action = "tool_call_end"
        );
        ToolResultMutation::default()
    }

    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        tracing::info!(
            target: "pandaria.audit",
            turn_index = ctx.turn_index,
            message_count = ctx.messages.len(),
            action = "turn_end"
        );
    }
}
```

- [ ] **Step 2: Create rate_limit.rs**

```rust
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::host::extension::{Extension, ToolCallMutation};
use agent_core::context::ToolCallCtx;
use agent_core::mutations::HookDecision;

pub struct RateLimitExtension {
    max_calls_per_minute: u64,
    call_times: Mutex<Vec<Instant>>,
}

impl RateLimitExtension {
    pub fn new(max_calls_per_minute: u64) -> Self {
        Self {
            max_calls_per_minute,
            call_times: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Extension for RateLimitExtension {
    fn name(&self) -> &str { "rate-limit" }

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        let mut times = self.call_times.lock().expect("rate-limit mutex poisoned");
        let now = Instant::now();

        times.retain(|t| now.duration_since(*t) < Duration::from_secs(60));

        if times.len() as u64 >= self.max_calls_per_minute {
            return (
                HookDecision::Block {
                    reason: format!(
                        "rate limit exceeded: {} tool calls per minute (limit: {})",
                        times.len(),
                        self.max_calls_per_minute
                    ),
                },
                ToolCallMutation::default(),
            );
        }

        times.push(now);
        (HookDecision::Continue, ToolCallMutation::default())
    }
}
```

- [ ] **Step 3: Create tool_guard.rs**

```rust
use async_trait::async_trait;

use crate::host::extension::{Extension, ToolCallMutation};
use agent_core::context::ToolCallCtx;
use agent_core::mutations::HookDecision;

pub struct ToolGuardExtension {
    allowed_tools: Vec<String>,
    denied_tools: Vec<String>,
}

impl ToolGuardExtension {
    pub fn new(allowed_tools: Vec<String>, denied_tools: Vec<String>) -> Self {
        Self { allowed_tools, denied_tools }
    }
}

#[async_trait]
impl Extension for ToolGuardExtension {
    fn name(&self) -> &str { "tool-guard" }

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        if self.denied_tools.contains(&ctx.tool_name) {
            return (
                HookDecision::Block {
                    reason: format!("tool '{}' is denied by tool-guard", ctx.tool_name),
                },
                ToolCallMutation::default(),
            );
        }

        if !self.allowed_tools.is_empty() && !self.allowed_tools.contains(&ctx.tool_name) {
            return (
                HookDecision::Block {
                    reason: format!(
                        "tool '{}' is not in allowed list ({:?})",
                        ctx.tool_name,
                        self.allowed_tools
                    ),
                },
                ToolCallMutation::default(),
            );
        }

        (HookDecision::Continue, ToolCallMutation::default())
    }
}
```

- [ ] **Step 4: Update builtins/mod.rs**

```rust
pub mod audit;
pub mod rate_limit;
pub mod tool_guard;

pub use audit::AuditExtension;
pub use rate_limit::RateLimitExtension;
pub use tool_guard::ToolGuardExtension;
```

- [ ] **Step 5: Verify compilation**

Run: `cargo build -p extensions`
Expected: Compiles successfully

- [ ] **Step 6: Commit**

```bash
git add crates/extensions/src/builtins/
git commit -m "feat(extensions): add audit, rate-limit, and tool-guard builtins"
```

---

## Task 10: Update lib.rs Exports

**Files:**
- Modify: `crates/extensions/src/lib.rs`

- [ ] **Step 1: Add new exports**

```rust
pub mod builtins;
pub mod host;

pub use host::extension::{Extension, ToolCallMutation};
pub use host::event_bus::{EventBus, ObservationEvent};
pub use host::hook_router::HookRouter;
pub use host::manager::ExtensionManager;
pub use host::extension_actor::{ExtensionHandle, ExtensionActor};
```

- [ ] **Step 2: Verify compilation**

Run: `cargo build -p extensions`
Expected: Compiles successfully

- [ ] **Step 3: Commit**

```bash
git add crates/extensions/src/lib.rs
git commit -m "feat(extensions): update public API exports"
```

---

## Task 11: Write HookRouter Tests

**Files:**
- Create: `crates/extensions/tests/hook_router_tests.rs`

- [ ] **Step 1: Write test file**

Create `crates/extensions/tests/hook_router_tests.rs` with tests for:

```rust
use std::sync::Arc;
use extensions::host::extension::{Extension, ToolCallMutation};
use extensions::host::extension_actor::ExtensionActor;
use extensions::host::event_bus::EventBus;
use extensions::host::hook_router::HookRouter;
use agent_core::context::{ToolCallCtx, ToolResultCtx, ContextCtx, CompactCtx, TurnEndCtx};
use agent_core::mutations::{HookDecision, ToolResultMutation, ContextMutation, CompactDecision};
use async_trait::async_trait;

// Test helpers
struct ContinueExt { name: String }
#[async_trait]
impl Extension for ContinueExt {
    fn name(&self) -> &str { &self.name }
}

struct BlockExt { name: String }
#[async_trait]
impl Extension for BlockExt {
    fn name(&self) -> &str { &self.name }
    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        (HookDecision::Block { reason: "blocked".to_string() }, ToolCallMutation::default())
    }
}

fn test_bus() -> Arc<EventBus> {
    Arc::new(EventBus::new(16))
}

fn test_tool_ctx() -> ToolCallCtx {
    ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    }
}

#[tokio::test]
async fn test_blocking_first_block_wins() {
    let bus = test_bus();
    let ext1 = Arc::new(ContinueExt { name: "ext1".to_string() });
    let ext2 = Arc::new(BlockExt { name: "ext2".to_string() });
    let ext3 = Arc::new(ContinueExt { name: "ext3".to_string() });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8, "t1".to_string(), "s1".to_string());
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8, "t1".to_string(), "s1".to_string());
    let (h3, _) = ExtensionActor::spawn(ext3, bus.clone(), 8, "t1".to_string(), "s1".to_string());

    let router = HookRouter::new(vec![h1, h2, h3], bus);
    let (decision, _) = router.on_tool_call(&test_tool_ctx()).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
}

#[tokio::test]
async fn test_blocking_all_continue() {
    let bus = test_bus();
    let ext1 = Arc::new(ContinueExt { name: "ext1".to_string() });
    let ext2 = Arc::new(ContinueExt { name: "ext2".to_string() });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8, "t1".to_string(), "s1".to_string());
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8, "t1".to_string(), "s1".to_string());

    let router = HookRouter::new(vec![h1, h2], bus);
    let (decision, _) = router.on_tool_call(&test_tool_ctx()).await;
    assert!(matches!(decision, HookDecision::Continue));
}

// Add 10+ more tests following spec §11.1
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p extensions -- hook_router_tests`
Expected: Tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/extensions/tests/hook_router_tests.rs
git commit -m "test(extensions): add HookRouter integration tests"
```

---

## Task 12: Write ExtensionActor Tests

**Files:**
- Create: `crates/extensions/tests/extension_actor_tests.rs`

- [ ] **Step 1: Write test file**

Create tests for startup/shutdown, panic isolation, timeout, and event bus handling.

- [ ] **Step 2: Run tests**

Run: `cargo test -p extensions -- extension_actor_tests`
Expected: Tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/extensions/tests/extension_actor_tests.rs
git commit -m "test(extensions): add ExtensionActor lifecycle and panic tests"
```

---

## Task 13: Write EventBus and Builtin Tests

**Files:**
- Create: `crates/extensions/tests/event_bus_tests.rs`
- Create: `crates/extensions/tests/builtin_audit_tests.rs`
- Create: `crates/extensions/tests/builtin_rate_limit_tests.rs`
- Create: `crates/extensions/tests/builtin_tool_guard_tests.rs`

- [ ] **Step 1: Write event_bus_tests.rs**

Test broadcast, lag handling, and no-subscriber warnings.

- [ ] **Step 2: Write builtin tests**

Follow spec §11.3 for audit (3 tests), rate-limit (3 tests), tool-guard (4 tests).

- [ ] **Step 3: Run all tests**

Run: `cargo test -p extensions`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/extensions/tests/
git commit -m "test(extensions): add EventBus and builtin extension tests"
```

---

## Task 14: Final Verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p extensions`
Expected: All tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p extensions`
Expected: No warnings (or only allowed ones)

- [ ] **Step 3: Check for placeholder cleanup**

If agent-core now provides the types, remove `crates/extensions/src/host/_placeholders.rs` and update imports.

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "feat(extensions): complete upgrade to full spec (14 hooks, builtins, tests)"
```

---

## Plan Review Loop

After completing this plan:

1. Dispatch a plan-document-reviewer subagent with:
   - Plan path: `docs/superpowers/plans/2026-05-03-extensions.md`
   - Spec path: `docs/specs/2026-05-02-extensions.md`
2. If issues found: fix and re-review
3. If approved: proceed to execution

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-05-03-extensions.md`. Two execution options:**

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**
