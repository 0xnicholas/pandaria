# extensions 详细模块规格

**Date:** 2026-05-02
**Status:** Draft
**Reference:** AGENTS.md (ADR-002 Extension trait, ADR-003 Hybrid Hook mechanism), pi.dev Extension system (`packages/coding-agent/src/core/extensions/`)

---

## 模块定位

实现 AGENTS.md ADR-002 的 `Extension` trait 和 ADR-003 的混合 Hook 机制（Actor Mailbox + EventBus）。为 `agent-core` 的 `HookDispatcher` trait 提供具体实现。

## 依赖方向

```
extensions → agent-core → ai-provider
```

---

## 1. 文件结构

```
crates/extensions/
  Cargo.toml
  README.md
  src/
    lib.rs                         # re-exports
    host/
      mod.rs
      extension.rs                 # Extension trait (ADR-002)
      extension_actor.rs           # ExtensionActor — 每个 extension 独立 tokio task
      hook_router.rs               # HookRouter — 实现 agent_core::HookDispatcher
      event_bus.rs                 # EventBus — broadcast::Sender 包装
      manager.rs                   # ExtensionManager — 生命周期、工具收集
      extension_tool.rs            # ExtensionTool — 将 Extension::tools() 包装为 AgentTool
    builtins/
      mod.rs
      audit.rs                     # 审计日志 extension
      rate_limit.rs                # 限流 extension
      tool_guard.rs                # 工具访问控制 extension
  tests/
    hook_router_tests.rs           # HookRouter dispatch tests
    extension_actor_tests.rs       # Actor lifecycle, panic isolation, timeout
    event_bus_tests.rs             # EventBus broadcast/receive
    builtin_audit_tests.rs         # Audit extension tests
    builtin_rate_limit_tests.rs    # Rate limit extension tests
    builtin_tool_guard_tests.rs    # Tool guard extension tests
```

---

## 2. 依赖

```toml
[dependencies]
agent-core = { path = "../agent-core" }
ai-provider = { path = "../ai-provider" }
tokio = { workspace = true, features = ["sync", "time", "rt", "macros"] }
async-trait = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tokio-test = "0.4"
```

---

## 3. Extension trait

`src/host/extension.rs`

```rust
use async_trait::async_trait;
use agent_core::context::{
    ToolCallCtx, ToolResultCtx, ContextCtx, TurnEndCtx, AgentEndCtx, SessionCtx,
    BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx,
    ToolExecutionStartCtx, ToolExecutionUpdateCtx, ToolExecutionEndCtx,
    CompactCtx, CompactEndCtx,
};
use agent_core::mutations::{
    HookDecision, ToolResultMutation, ContextMutation,
    BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation,
    CompactDecision,
};
use agent_core::error::AgentError;
use agent_core::types::AgentToolResult;
use llm_client::ToolDef;

/// Mutation returned by blocking hooks for tool calls.
/// Unlike other blocking hooks, on_tool_call supports input mutation
/// to enable parameter sanitization, injection, and transformation.
#[derive(Debug, Clone, Default)]
pub struct ToolCallMutation {
    /// Replaced tool input parameters. When Some, replaces the original
    /// `ctx.input` for subsequent handlers and tool execution.
    pub input: Option<serde_json::Value>,
}

/// Extension trait — the abstract boundary for all extension implementations.
///
/// Each hook method has a default empty implementation. Extensions override
/// only the hooks they need.
///
/// ## Hook Transport (per ADR-003)
///
/// | Hook | Transport | Execution | Merge |
/// |---|---|---|---|
/// | `on_tool_call` | Actor Mailbox + oneshot | Serial, waits | first-block-wins + input mutation chain |
/// | `on_tool_result` | Actor Mailbox + oneshot | Serial, waits | chain merge |
/// | `on_context` | Actor Mailbox + oneshot | Serial, waits | chain merge |
/// | `on_before_agent_start` | Actor Mailbox + oneshot | Serial, waits | chain merge |
/// | `on_before_provider_request` | Actor Mailbox + oneshot | Serial, waits | chain merge |
/// | `on_after_provider_response` | Actor Mailbox + oneshot | Serial, waits | chain merge |
/// | `on_before_compact` | Actor Mailbox + oneshot | Serial, waits | first-block-wins |
/// | `execute_tool` | Actor Mailbox + oneshot | Spawned, no timeout | completion result |
/// | `on_turn_end` | EventBus broadcast | Concurrent, fire-and-forget | none |
/// | `on_agent_end` | EventBus broadcast | Concurrent, fire-and-forget | none |
/// | `on_session_start` | EventBus broadcast | Concurrent, fire-and-forget | none |
/// | `on_tool_execution_start` | EventBus broadcast | Concurrent, fire-and-forget | none |
/// | `on_tool_execution_update` | EventBus broadcast | Concurrent, fire-and-forget | none |
/// | `on_tool_execution_end` | EventBus broadcast | Concurrent, fire-and-forget | none |
/// | `on_compact_end` | EventBus broadcast | Concurrent, fire-and-forget | none |
#[async_trait]
pub trait Extension: Send + Sync {
    /// Unique extension name. Used for logging, metrics, and routing.
    fn name(&self) -> &str;

    /// Tool definitions this extension contributes to the agent.
    /// Default: no tools.
    fn tools(&self) -> Vec<ToolDef> {
        vec![]
    }

    // ═══ Blocking hooks — first-block-wins ═══

    /// Blocking hook with input mutation support.
    /// Returns (decision, mutation). Even when Block is returned,
    /// accumulated mutations from previous handlers are preserved.
    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        (HookDecision::Continue, ToolCallMutation::default())
    }

    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Continue
    }

    // ═══ Chaining hooks — chain merge ═══

    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        ToolResultMutation::default()
    }

    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
        ContextMutation::default()
    }

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

    /// Execute a tool registered by this extension.
    ///
    /// Called when the LLM invokes a tool whose name matches one of this
    /// extension's `tools()` definitions. Transport: Actor Mailbox + oneshot.
    /// Unlike blocking/chain hooks, tool execution has NO framework-imposed
    /// timeout — the tool runs to completion (matching `AgentTool::execute` semantics).
    ///
    /// Default: returns an error (extension tools are non-executable unless
    /// overridden).
    async fn execute_tool(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
        Err(AgentError::ToolExecutionFailed(
            "tool defined but not executable by this extension".into(),
        ))
    }

    // ═══ Observational hooks — fire-and-forget ═══

    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {}
    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {}
    async fn on_session_start(&self, _ctx: &SessionCtx) {}
    async fn on_tool_execution_start(&self, _ctx: &ToolExecutionStartCtx) {}
    async fn on_tool_execution_update(&self, _ctx: &ToolExecutionUpdateCtx) {}
    async fn on_tool_execution_end(&self, _ctx: &ToolExecutionEndCtx) {}
    async fn on_compact_end(&self, _ctx: &CompactEndCtx) {}
}
```

---

## 4. ExtensionActor

`src/host/extension_actor.rs`

每个 `Extension` 实例运行在独立 tokio task 中。Actor 通过 `tokio::mpsc` 接收命令（阻塞/链式 hook），通过 `tokio::sync::broadcast` 接收观测事件。

### 4.1 Mailbox 命令

```rust
use tokio::sync::oneshot;

/// Commands sent to an ExtensionActor via its mpsc mailbox.
enum ExtensionCommand {
    // Blocking hooks (first-block-wins)
    OnToolCall {
        ctx: ToolCallCtx,
        reply: oneshot::Sender<(HookDecision, ToolCallMutation)>,
    },
    OnBeforeCompact {
        ctx: CompactCtx,
        reply: oneshot::Sender<CompactDecision>,
    },

    // Chaining hooks (chain merge)
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

    // Tool execution — no timeout, runs to completion
    OnExecuteTool {
        tool_call_id: String,
        params: serde_json::Value,
        reply: oneshot::Sender<Result<AgentToolResult, AgentError>>,
    },

    /// Graceful shutdown — actor exits its loop.
    Shutdown,
}
```

### 4.2 Actor 结构

```rust
struct ExtensionActor {
    extension: Arc<dyn Extension>,
    mailbox: mpsc::Receiver<ExtensionCommand>,
    event_bus_rx: broadcast::Receiver<ObservationEvent>,
    tenant_id: String,
    session_id: String,
}
```

### 4.3 Actor 循环

```rust
impl ExtensionActor {
    async fn run(mut self) {
        loop {
            tokio::select! {
                // Mailbox commands (blocking / chaining hooks)
                cmd = self.mailbox.recv() => {
                    match cmd {
                        // Blocking hooks
                        Some(ExtensionCommand::OnToolCall { ctx, reply }) => {
                            self.handle_blocking("on_tool_call", ctx, reply).await;
                        }
                        Some(ExtensionCommand::OnBeforeCompact { ctx, reply }) => {
                            self.handle_blocking("on_before_compact", ctx, reply).await;
                        }
                        // Chaining hooks
                        Some(ExtensionCommand::OnToolResult { ctx, reply }) => {
                            self.handle_chain("on_tool_result", ctx, reply).await;
                        }
                        Some(ExtensionCommand::OnContext { ctx, reply }) => {
                            self.handle_chain("on_context", ctx, reply).await;
                        }
                        Some(ExtensionCommand::OnBeforeAgentStart { ctx, reply }) => {
                            self.handle_chain("on_before_agent_start", ctx, reply).await;
                        }
                        Some(ExtensionCommand::OnBeforeProviderRequest { ctx, reply }) => {
                            self.handle_chain("on_before_provider_request", ctx, reply).await;
                        }
                        Some(ExtensionCommand::OnAfterProviderResponse { ctx, reply }) => {
                            self.handle_chain("on_after_provider_response", ctx, reply).await;
                        }
                        // Tool execution — fire-and-forget spawn, no timeout
                        Some(ExtensionCommand::OnExecuteTool { tool_call_id, params, reply }) => {
                            let ext = self.extension.clone();
                            tokio::spawn(async move {
                                let result = ext.execute_tool(&tool_call_id, params).await;
                                let _ = reply.send(result);
                            });
                        }
                        Some(ExtensionCommand::Shutdown) | None => break,
                    }
                }
                // Observation events (fire-and-forget)
                Ok(event) = self.event_bus_rx.recv() => {
                    // `Lagged(n)` events cause the recv() pattern to not match,
                    // so the actor implicitly skips and retries on the next iteration.
                    self.handle_observation(event).await;
                }
            }
        }
    }

The `event_bus_rx.recv()` in the actor loop may return `RecvError::Lagged(n)` when
the broadcasts channel evicts messages before this receiver has consumed them.
When this occurs, the `Ok(event)` pattern in `select!` does not match, and the
actor loops back to the next poll — effectively skipping the lost messages without
logging (the actor recovers automatically). Implementations SHOULD add
`tracing::warn!` for `Lagged` errors to surface capacity pressure to operators
(see Section 5.1 for loss tolerance per event category).

    /// Handle a blocking hook: execute via tokio::spawn for panic isolation.
    /// Generic over the reply type — supports both HookDecision and CompactDecision.
    async fn handle_blocking<T>(
        &self,
        hook_name: &str,
        ctx: T,
        reply: oneshot::Sender<T::Output>,
    ) where
        T: Send + 'static + Clone,
        Extension: HookByName<T>,
        T::Output: Default + Send + 'static,
    {
        let extension = self.extension.clone();
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();
        let name = self.extension.name().to_string();

        // Spawn to catch panics (tokio::spawn catches and returns JoinError).
        let handle = tokio::spawn(async move {
            let span = tracing::info_span!(
                "extension_hook",
                extension = %name,
                hook = hook_name,
                tenant_id = %tenant_id,
                session_id = %session_id,
            );
            let _enter = span.enter();
            Extension::dispatch(&*extension, hook_name, ctx).await
        });

        match handle.await {
            Ok(result) => { let _ = reply.send(result); }
            Err(join_err) => {
                tracing::error!(
                    extension = %self.extension.name(),
                    hook = hook_name,
                    "extension panicked in blocking hook, returning default"
                );
                if let Ok(panic_msg) = join_err.try_into_panic()
                    .and_then(|p| p.downcast_ref::<String>().cloned())
                {
                    tracing::error!(panic_msg = %panic_msg, "panic detail");
                }
                let _ = reply.send(T::Output::default());
            }
        }
    }

    /// Handle a chain hook: execute via tokio::spawn for panic isolation.
    async fn handle_chain<T>(
        &self,
        hook_name: &str,
        ctx: T,
        reply: oneshot::Sender<T::Mutation>,
    ) where
        T: Send + 'static + Clone,
        Extension: HookByName<T, Output = T::Mutation>,
        T::Mutation: Default + Send + 'static,
    {
        let extension = self.extension.clone();
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();
        let name = self.extension.name().to_string();

        let handle = tokio::spawn(async move {
            let span = tracing::info_span!(
                "extension_hook",
                extension = %name,
                hook = hook_name,
                tenant_id = %tenant_id,
                session_id = %session_id,
            );
            let _enter = span.enter();
            Extension::dispatch(&*extension, hook_name, ctx).await
        });

        match handle.await {
            Ok(result) => { let _ = reply.send(result); }
            Err(_panic) => {
                tracing::error!(
                    extension = %self.extension.name(),
                    hook = hook_name,
                    "extension panicked in chain hook, returning default"
                );
                let _ = reply.send(T::Mutation::default());
            }
        }
    }

    /// Handle an observation event: spawn fire-and-forget with 100ms timeout.
    /// Dispatches to the correct Extension method based on the event type.
    /// Panics are caught by tokio::spawn; timeouts silently drop.
    async fn handle_observation(&self, event: ObservationEvent) {
        let extension = self.extension.clone();
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();
        let name = self.extension.name().to_string();
        let (hook_name, owned_ctx) = event.into_parts();

        // Fire-and-forget: do NOT await the JoinHandle (non-blocking per ADR-003).
        tokio::spawn(async move {
            let span = tracing::info_span!(
                "extension_hook",
                extension = %name,
                hook = hook_name,
                tenant_id = %tenant_id,
                session_id = %session_id,
            );
            let _enter = span.enter();

            let result = tokio::time::timeout(
                Duration::from_millis(100),
                async {
                    match owned_ctx {
                        OwnedObsCtx::TurnEnd(ctx) => extension.on_turn_end(&ctx).await,
                        OwnedObsCtx::AgentEnd(ctx) => extension.on_agent_end(&ctx).await,
                        OwnedObsCtx::SessionStart(ctx) => extension.on_session_start(&ctx).await,
                        OwnedObsCtx::ToolExecutionStart(ctx) => extension.on_tool_execution_start(&ctx).await,
                        OwnedObsCtx::ToolExecutionUpdate(ctx) => extension.on_tool_execution_update(&ctx).await,
                        OwnedObsCtx::ToolExecutionEnd(ctx) => extension.on_tool_execution_end(&ctx).await,
                        OwnedObsCtx::CompactEnd(ctx) => extension.on_compact_end(&ctx).await,
                    }
                },
            ).await;

            if result.is_err() {
                tracing::warn!(
                    extension = %name,
                    hook = hook_name,
                    "observation hook timed out after 100ms, silently dropped"
                );
            }
        });
    }
}
```

### 4.4 Hook 分发辅助 trait

```rust
/// Dispatch a blocking or chain hook by name on an Extension trait object.
/// Avoids duplicating match statements in the actor loop.
trait HookByName<Ctx> {
    type Output;
    async fn dispatch(ext: &dyn Extension, hook: &str, ctx: Ctx) -> Self::Output;
    async fn dispatch_observation(ext: &dyn Extension, hook: &str, ctx: &Ctx);
}

impl HookByName<ToolCallCtx> for dyn Extension {
    type Output = (HookDecision, ToolCallMutation);
    async fn dispatch(ext: &dyn Extension, _hook: &str, ctx: ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        ext.on_tool_call(&ctx).await
    }
    async fn dispatch_observation(ext: &dyn Extension, _hook: &str, _ctx: &ToolCallCtx) {}
}

impl HookByName<CompactCtx> for dyn Extension {
    type Output = CompactDecision;
    async fn dispatch(ext: &dyn Extension, _hook: &str, ctx: CompactCtx) -> CompactDecision {
        ext.on_before_compact(&ctx).await
    }
    async fn dispatch_observation(ext: &dyn Extension, _hook: &str, _ctx: &CompactCtx) {}
}

impl HookByName<ToolResultCtx> for dyn Extension {
    type Output = ToolResultMutation;
    async fn dispatch(ext: &dyn Extension, _hook: &str, ctx: ToolResultCtx) -> ToolResultMutation {
        ext.on_tool_result(&ctx).await
    }
    async fn dispatch_observation(ext: &dyn Extension, _hook: &str, _ctx: &ToolResultCtx) {}
}

impl HookByName<ContextCtx> for dyn Extension {
    type Output = ContextMutation;
    async fn dispatch(ext: &dyn Extension, _hook: &str, ctx: ContextCtx) -> ContextMutation {
        ext.on_context(&ctx).await
    }
    async fn dispatch_observation(ext: &dyn Extension, _hook: &str, _ctx: &ContextCtx) {}
}

impl HookByName<BeforeAgentStartCtx> for dyn Extension {
    type Output = BeforeAgentStartMutation;
    async fn dispatch(ext: &dyn Extension, _hook: &str, ctx: BeforeAgentStartCtx) -> BeforeAgentStartMutation {
        ext.on_before_agent_start(&ctx).await
    }
    async fn dispatch_observation(ext: &dyn Extension, _hook: &str, _ctx: &BeforeAgentStartCtx) {}
}

impl HookByName<ProviderRequestCtx> for dyn Extension {
    type Output = ProviderRequestMutation;
    async fn dispatch(ext: &dyn Extension, _hook: &str, ctx: ProviderRequestCtx) -> ProviderRequestMutation {
        ext.on_before_provider_request(&ctx).await
    }
    async fn dispatch_observation(ext: &dyn Extension, _hook: &str, _ctx: &ProviderRequestCtx) {}
}

impl HookByName<ProviderResponseCtx> for dyn Extension {
    type Output = ProviderResponseMutation;
    async fn dispatch(ext: &dyn Extension, _hook: &str, ctx: ProviderResponseCtx) -> ProviderResponseMutation {
        ext.on_after_provider_response(&ctx).await
    }
    async fn dispatch_observation(ext: &dyn Extension, _hook: &str, _ctx: &ProviderResponseCtx) {}
}
```

ObservationEvent 也需要转换为 owned ctx：

```rust
impl ObservationEvent {
    fn into_parts(self) -> (&'static str, OwnedObsCtx) {
        match self {
            ObservationEvent::TurnEnd(ctx) => ("on_turn_end", OwnedObsCtx::TurnEnd(ctx)),
            ObservationEvent::AgentEnd(ctx) => ("on_agent_end", OwnedObsCtx::AgentEnd(ctx)),
            ObservationEvent::SessionStart(ctx) => ("on_session_start", OwnedObsCtx::SessionStart(ctx)),
            ObservationEvent::ToolExecutionStart(ctx) => ("on_tool_execution_start", OwnedObsCtx::ToolExecutionStart(ctx)),
            ObservationEvent::ToolExecutionUpdate(ctx) => ("on_tool_execution_update", OwnedObsCtx::ToolExecutionUpdate(ctx)),
            ObservationEvent::ToolExecutionEnd(ctx) => ("on_tool_execution_end", OwnedObsCtx::ToolExecutionEnd(ctx)),
            ObservationEvent::CompactEnd(ctx) => ("on_compact_end", OwnedObsCtx::CompactEnd(ctx)),
        }
    }
}

enum OwnedObsCtx {
    TurnEnd(TurnEndCtx),
    AgentEnd(AgentEndCtx),
    SessionStart(SessionCtx),
    ToolExecutionStart(ToolExecutionStartCtx),
    ToolExecutionUpdate(ToolExecutionUpdateCtx),
    ToolExecutionEnd(ToolExecutionEndCtx),
    CompactEnd(CompactEndCtx),
}
```

### 4.5 ExtensionHandle

```rust
/// External handle to an ExtensionActor's mailbox.
/// Held by HookRouter for sending commands.
#[derive(Clone)]
struct ExtensionHandle {
    name: String,
    sender: mpsc::Sender<ExtensionCommand>,
}

impl ExtensionHandle {
    /// Send a command and await the oneshot reply with timeout.
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

    /// Execute a tool via this extension's actor. No timeout — tool execution
    /// can be long-running (e.g. bash commands). Sends OnExecuteTool, awaits result.
    async fn execute_tool(
        &self,
        tool_call_id: String,
        params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
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

enum AskError {
    Timeout,
    ActorGone,
}
```

---

## 5. EventBus

`src/host/event_bus.rs`

```rust
use tokio::sync::broadcast;

/// Observation events delivered via broadcast to all extension actors.
#[derive(Debug, Clone)]
enum ObservationEvent {
    TurnEnd(TurnEndCtx),
    AgentEnd(AgentEndCtx),
    SessionStart(SessionCtx),
    ToolExecutionStart(ToolExecutionStartCtx),
    ToolExecutionUpdate(ToolExecutionUpdateCtx),
    ToolExecutionEnd(ToolExecutionEndCtx),
    CompactEnd(CompactEndCtx),
}

struct EventBus {
    tx: broadcast::Sender<ObservationEvent>,
}

impl EventBus {
    /// Create a new EventBus. `capacity` is the broadcast channel buffer size.
    fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Emit an event to all subscribers. Non-blocking — uses `broadcast::Sender::send()`.
    ///
    /// Behavior of `broadcast::Sender::send()`:
    /// - With >= 1 active receiver: `send()` always succeeds. Tokio's broadcast
    ///   channel evicts the oldest buffered message to make room. Slow receivers
    ///   see `RecvError::Lagged(n)` on their next `recv()` — missed events are
    ///   permanently lost for that receiver. This is the normal operating mode.
    /// - With zero active receivers: `send()` returns `Err(SendError)`. The event
    ///   is dropped with a `tracing::warn!` since no consumer exists.
    fn emit(&self, event: ObservationEvent) {
        if self.tx.send(event).is_err() {
            tracing::warn!(
                event = %event.variant_name(),
                "EventBus emit failed: no active subscribers, event dropped"
            );
        }
    }

    /// Create a new subscriber receiver.
    fn subscribe(&self) -> broadcast::Receiver<ObservationEvent> {
        self.tx.subscribe()
    }
}
```

### 5.1 Event Loss Tolerance

Observation events have different semantic importance. The EventBus treats all events
as fire-and-forget, but consumers SHOULD prioritize delivery assurance for critical
events.

| Category | Events | Loss Tolerance | Rationale |
|---|---|---|---|
| **lifecycle** | `SessionStart`, `AgentEnd` | should not lose | Session/agent lifecycle boundaries required for audit completeness, billing, and session recovery. Loss of these events creates permanent gaps in the audit trail. |
| **state-change** | `TurnEnd`, `ToolExecutionEnd`, `CompactEnd` | may lose occasionally | Important for observability but reconstructable from turn history and tool result messages. |
| **best-effort** | `ToolExecutionStart`, `ToolExecutionUpdate` | may lose frequently | High-frequency process events (e.g., streaming tool output). Best-effort observability only; extensions MUST NOT rely on receiving every event in this category. |

**Design implications:**

- The EventBus channel capacity should be sized for lifecycle + state-change events
  combined, assuming best-effort events may saturate the remaining buffer.
- Extensions that require guaranteed delivery of best-effort events should implement
  their own buffering strategy (e.g., sampling, ring buffer with aggregation).
- Slow extensions (ones that take >100ms to process an observation event) will see
  `RecvError::Lagged(n)` on the receive side. For best-effort events this is
  acceptable; for lifecycle events it indicates a misconfigured extension or
  insufficient EventBus capacity.
- A future revision MAY introduce separate high-priority and low-priority broadcast
  channels to physically partition lifecycle events from best-effort events, but
  v0.1 treats all events as a single logical priority with different loss tolerance
  expectations.

---

## 6. HookRouter

`src/host/hook_router.rs`

实现 `agent_core::hook_dispatcher::HookDispatcher` trait。这是 agent-core 和 extensions 之间的依赖反转边界。

```rust
use agent_core::hook_dispatcher::HookDispatcher;
use agent_core::context::*;
use agent_core::mutations::*;

struct HookRouter {
    extensions: Vec<ExtensionHandle>,       // ordered by registration
    event_bus: Arc<EventBus>,
    blocking_timeout: Duration,              // 500ms
    observation_timeout: Duration,           // 100ms
}
```

### 6.1 阻断型 — on_tool_call（支持 input mutation）

```rust
#[async_trait]
impl HookDispatcher for HookRouter {
    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        let mut current_ctx = ctx.clone();
        for handle in &self.extensions {
            let (decision, mutation) = match handle.ask::<(HookDecision, ToolCallMutation)>(
                |reply| ExtensionCommand::OnToolCall { ctx: current_ctx.clone(), reply },
                self.blocking_timeout,
            ).await {
                Ok(result) => result,
                Err(AskError::Timeout) => {
                    tracing::warn!(
                        extension = %handle.name,
                        "on_tool_call timeout after {:?}, treating as Continue",
                        self.blocking_timeout
                    );
                    (HookDecision::Continue, ToolCallMutation::default())
                }
                Err(AskError::ActorGone) => {
                    tracing::error!(extension = %handle.name, "ExtensionActor terminated unexpectedly");
                    (HookDecision::Continue, ToolCallMutation::default())
                }
            };

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

**合并语义：**
- `input` mutation 链式累积：每个 handler 看到的 `ctx.input` 是前面 handler 修改后的值
- 即使某个 handler 返回 `Block`，前面 handler 的 mutation 仍然保留并回传
- 调用方（`ToolExecutor`）使用返回的 `ToolCallMutation` 中的 `input` 替换原始参数执行工具

### 6.2 链式 — on_tool_result

```rust
    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
        let mut accumulated = ToolResultMutation::default();
        let mut current_ctx = ctx.clone();

        for handle in &self.extensions {
            match handle.ask::<ToolResultMutation>(
                |reply| ExtensionCommand::OnToolResult { ctx: current_ctx.clone(), reply },
                self.blocking_timeout,
            ).await {
                Ok(mutation) => {
                    // Apply mutation to current_ctx for the next handler
                    apply_tool_result_mutation(&mut current_ctx, &mutation);
                    // Merge into accumulated result
                    merge_tool_result_mutation(&mut accumulated, &mutation);
                }
                Err(AskError::Timeout) => {
                    tracing::warn!(
                        extension = %handle.name,
                        "on_tool_result timeout, skipping handler"
                    );
                    continue;
                }
                Err(AskError::ActorGone) => {
                    tracing::error!(extension = %handle.name, "ExtensionActor terminated");
                    continue;
                }
            }
        }
        accumulated
    }
```

Mutation 合并规则：

```rust
/// Apply mutation fields to ctx. Only fields present in mutation are applied.
fn apply_tool_result_mutation(ctx: &mut ToolResultCtx, mutation: &ToolResultMutation) {
    if let Some(ref content) = mutation.content {
        ctx.content = content.clone();
    }
    if let Some(ref details) = mutation.details {
        ctx.details = Some(details.clone());
    }
    if let Some(is_error) = mutation.is_error {
        ctx.is_error = is_error;
    }
}

/// Merge mutation into accumulated result. Non-None fields overwrite.
fn merge_tool_result_mutation(acc: &mut ToolResultMutation, mutation: &ToolResultMutation) {
    if mutation.content.is_some() { acc.content = mutation.content.clone(); }
    if mutation.details.is_some() { acc.details = mutation.details.clone(); }
    if mutation.is_error.is_some() { acc.is_error = mutation.is_error; }
}

/// Apply ProviderRequestMutation to ctx in-place for next handler.
fn apply_provider_request_mutation(ctx: &mut ProviderRequestCtx, mutation: &ProviderRequestMutation) {
    if let Some(ref sp) = mutation.system_prompt {
        ctx.system_prompt = sp.clone();
    }
    if let Some(ref msgs) = mutation.messages {
        ctx.messages = msgs.clone();
    }
    if let Some(ref tools) = mutation.tools {
        ctx.tools = tools.clone();
    }
    if let Some(ref options) = mutation.options {
        ctx.options = options.clone();
    }
}

/// Merge ProviderRequestMutation into accumulated result. Non-None fields overwrite.
fn merge_provider_request_mutation(acc: &mut ProviderRequestMutation, mutation: &ProviderRequestMutation) {
    if mutation.system_prompt.is_some() { acc.system_prompt = mutation.system_prompt.clone(); }
    if mutation.messages.is_some() { acc.messages = mutation.messages.clone(); }
    if mutation.tools.is_some() { acc.tools = mutation.tools.clone(); }
    if mutation.options.is_some() { acc.options = mutation.options.clone(); }
}
```

### 6.3 链式 — on_context

```rust
    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        let mut accumulated = ContextMutation::default();
        let mut current_ctx = ctx.clone();

        for handle in &self.extensions {
            match handle.ask::<ContextMutation>(
                |reply| ExtensionCommand::OnContext { ctx: current_ctx.clone(), reply },
                self.blocking_timeout,
            ).await {
                Ok(mutation) => {
                    // Apply to current_ctx for next handler chaining
                    if let Some(ref msgs) = mutation.messages {
                        current_ctx.messages = msgs.clone();
                    }
                    // Merge into accumulated (last non-None wins)
                    if mutation.messages.is_some() { accumulated.messages = mutation.messages.clone(); }
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

### 6.4 观测型 — on_turn_end / on_agent_end / on_session_start / on_tool_execution_* / on_compact_end

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

### 6.5 阻断型 — on_before_compact

```rust
    async fn on_before_compact(&self, ctx: &CompactCtx) -> CompactDecision {
        // Clone ctx per handler for first-block-wins semantics
        for handle in &self.extensions {
            match handle.ask::<CompactDecision>(
                |reply| ExtensionCommand::OnBeforeCompact { ctx: ctx.clone(), reply },
                self.blocking_timeout,
            ).await {
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

### 6.6 链式 — on_before_agent_start

```rust
    async fn on_before_agent_start(&self, ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
        let mut accumulated = BeforeAgentStartMutation::default();
        let mut current_ctx = ctx.clone();

        for handle in &self.extensions {
            match handle.ask::<BeforeAgentStartMutation>(
                |reply| ExtensionCommand::OnBeforeAgentStart { ctx: current_ctx.clone(), reply },
                self.blocking_timeout,
            ).await {
                Ok(mutation) => {
                    // Apply to current_ctx for next handler chaining
                    if let Some(ref sp) = mutation.system_prompt {
                        current_ctx.system_prompt = Some(sp.clone());
                    }
                    if let Some(ref msgs) = mutation.messages {
                        current_ctx.messages = msgs.clone();
                    }
                    // Merge into accumulated (last non-None wins for each field)
                    if mutation.system_prompt.is_some() { accumulated.system_prompt = mutation.system_prompt.clone(); }
                    if mutation.messages.is_some() { accumulated.messages = mutation.messages.clone(); }
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

### 6.7 链式 — on_before_provider_request

```rust
    async fn on_before_provider_request(&self, ctx: &ProviderRequestCtx) -> ProviderRequestMutation {
        let mut accumulated = ProviderRequestMutation::default();
        let mut current_ctx = ctx.clone();

        for handle in &self.extensions {
            match handle.ask::<ProviderRequestMutation>(
                |reply| ExtensionCommand::OnBeforeProviderRequest { ctx: current_ctx.clone(), reply },
                self.blocking_timeout,
            ).await {
                Ok(mutation) => {
                    apply_provider_request_mutation(&mut current_ctx, &mutation);
                    merge_provider_request_mutation(&mut accumulated, &mutation);
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

### 6.8 链式 — on_after_provider_response

```rust
    async fn on_after_provider_response(&self, ctx: &ProviderResponseCtx) -> ProviderResponseMutation {
        let mut accumulated = ProviderResponseMutation::default();
        let mut current_ctx = ctx.clone();

        for handle in &self.extensions {
            match handle.ask::<ProviderResponseMutation>(
                |reply| ExtensionCommand::OnAfterProviderResponse { ctx: current_ctx.clone(), reply },
                self.blocking_timeout,
            ).await {
                Ok(mutation) => {
                    if let Some(ref content) = mutation.content {
                        current_ctx.content = content.clone();
                    }
                    if let Some(ref stop_reason) = mutation.stop_reason {
                        current_ctx.stop_reason = stop_reason.clone();
                    }
                    if mutation.content.is_some() { accumulated.content = mutation.content.clone(); }
                    if mutation.stop_reason.is_some() { accumulated.stop_reason = mutation.stop_reason.clone(); }
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
}
```

---

## 7. ExtensionManager

`src/host/manager.rs`

```rust
struct ExtensionManager {
    extensions: Vec<Arc<dyn Extension>>,
    event_bus_capacity: usize,  // default: 128
}

impl ExtensionManager {
    /// Create manager with ordered list of extensions.
    /// Order determines priority for blocking/chain hooks.
    fn new(extensions: Vec<Arc<dyn Extension>>) -> Self {
        Self {
            extensions,
            event_bus_capacity: 128,
        }
    }

    /// Collect all tool definitions. First-registration-wins (dedup by name).
    fn collect_tools(&self) -> Vec<ToolDef> {
        let mut seen = std::collections::HashSet::new();
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

    /// Spawn all ExtensionActors.
    /// Returns:
    ///   - HookRouter: implements HookDispatcher, ready for agent-core
    ///   - ExtensionHandles: for constructing ExtensionTool wrappers
    ///   - JoinHandles: for graceful shutdown (see `shutdown_all()`)
    fn spawn_all(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> (HookRouter, Vec<ExtensionHandle>, Vec<tokio::task::JoinHandle<()>>) {
        let event_bus = Arc::new(EventBus::new(self.event_bus_capacity));
        let mut handles = Vec::new();
        let mut ext_handles = Vec::new();
        let mut join_handles = Vec::new();

        for ext in &self.extensions {
            let (tx, rx) = mpsc::channel(32);
            let actor = ExtensionActor {
                extension: ext.clone(),
                mailbox: rx,
                event_bus_rx: event_bus.subscribe(),
                tenant_id: tenant_id.to_string(),
                session_id: session_id.to_string(),
            };

            let name = ext.name().to_string();
            let join_handle = tokio::spawn(actor.run());
            join_handles.push(join_handle);
            ext_handles.push(ExtensionHandle { name, sender: tx });
        }

        let hook_router = HookRouter {
            extensions: ext_handles.clone(),
            event_bus,
            blocking_timeout: Duration::from_millis(500),
            observation_timeout: Duration::from_millis(100),
        };

        (hook_router, ext_handles, join_handles)
    }

    // --- event_bus_capacity sizing guidance ---
    //
    // Base formula: 2 × N_extensions × avg_events_per_turn × concurrent_turns
    //   For 5 extensions × ~5 observation events/turn × 1 concurrent turn = 50.
    //   Add headroom for burst and best-effort event churn → 128.
    //
    // If best-effort events (ToolExecutionUpdate) are high-frequency (e.g.,
    // streaming tool output at 10Hz), consider increasing capacity or using
    // a dedicated low-priority broadcast channel in a future revision to avoid
    // evicting lifecycle events from the primary bus.
    //
    // See Section 5.1 for per-category loss tolerance.

    /// Wrap extension tool definitions into AgentToolRef objects.
    ///
    /// For each extension that provides `tools()`, creates an `ExtensionTool`
    /// wrapper that delegates execution to `ExtensionHandle::execute_tool()`.
    ///
    /// Merge strategy: first-registration-wins per tool name (same as `collect_tools()`).
    /// The caller should merge these with native `AgentToolRef`s — extension tools
    /// overwrite native tools by name.
    fn collect_agent_tools(handles: &[ExtensionHandle], extensions: &[Arc<dyn Extension>]) -> Vec<AgentToolRef> {
        let mut seen = std::collections::HashSet::new();
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
                            execution_mode: ToolExecutionMode::Parallel,
                        }) as AgentToolRef);
                    }
                }
            }
        }
        tools
    }
}
```

---

## 8. ExtensionTool (`src/host/extension_tool.rs`)

将 Extension 的 `tools()` 定义和 `execute_tool()` 执行器包装为 `AgentTool` trait 对象，使 AgentLoop 能像对待原生工具一样调用扩展工具。

```rust
use std::sync::Arc;
use agent_core::types::{AgentTool, AgentToolProgressUpdate, AgentToolResult, AgentToolRef, ToolExecutionMode};
use agent_core::error::AgentError;

/// Wraps an Extension-registered tool into an AgentTool that delegates
/// execution to the ExtensionActor via ExtensionHandle::execute_tool().
///
/// Multiple ExtensionTool instances may hold the same ExtensionHandle
/// (one per tool name registered by the same extension).
struct ExtensionTool {
    name: String,
    description: String,
    parameters: serde_json::Value,
    handle: ExtensionHandle,
    execution_mode: ToolExecutionMode,
}

#[async_trait::async_trait]
impl AgentTool for ExtensionTool {
    fn name(&self) -> &str { &self.name }

    fn description(&self) -> &str { &self.description }

    fn parameters(&self) -> serde_json::Value { self.parameters.clone() }

    fn execution_mode(&self) -> ToolExecutionMode { self.execution_mode }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
    ) -> Result<AgentToolResult, AgentError> {
        // v0.1: no progress streaming for extension tools.
        // The framework's on_tool_execution_update (observational hook) still fires
        // for monitoring purposes via the ToolExecutor pipeline.
        self.handle.execute_tool(tool_call_id.to_string(), params).await
    }
}
```

### 生命周期

```
ExtensionManager::spawn_all()
  └── returns ExtensionHandle[]
        │
ExtensionManager::collect_agent_tools(&handles)
  └── ExtensionTool { handle: handle.clone(), ... } for each ext.tools() entry
        │
Wiring layer
  └── merge(native_agent_tools, extension_agent_tools)  // ext overwrites by name
        │
SessionActor::new(..., merged_tools)
        │
AgentLoop::run()
  └── LLM calls tool "ext_x"
        │
ToolExecutor::execute_tool_call()
  ├── on_tool_call hook (blocking — fires for ALL extensions)
  ├── ExtensionTool::execute() → ExtensionHandle::execute_tool()
  │     └── Mailbox → ExtensionActor → tokio::spawn → ext.execute_tool()
  └── on_tool_result hook (chain — fires for ALL extensions)
```

### 关键约束

- Extension 工具的 `execute_tool()` 通过 Mailbox 无超时执行（与 `on_tool_call` / `on_tool_result` 的 500ms 不同）
- 进度回调（`on_progress`）v0.1 不支持——Extension 工具一次性返回结果
- `on_tool_execution_*` 观测型 hook 仍正常触发（由 ToolExecutor 发出）

---

## 9. 内置 Extension

### 9.1 AuditExtension

`src/builtins/audit.rs`

记录所有 tool call 和 turn 事件到 tracing journal。

```rust
struct AuditExtension;

impl Extension for AuditExtension {
    fn name(&self) -> &str { "audit" }

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        tracing::info!(
            target: "pandaria.audit",
            tool_name = %ctx.tool_name,
            tool_call_id = %ctx.tool_call_id,
            action = "tool_call_start"
        );
        (HookDecision::Continue, ToolCallMutation::default())  // never blocks, never mutates
    }

    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
        tracing::info!(
            target: "pandaria.audit",
            tool_name = %ctx.tool_name,
            tool_call_id = %ctx.tool_call_id,
            is_error = ctx.is_error,
            action = "tool_call_end"
        );
        ToolResultMutation::default()  // never mutates results
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

### 9.2 RateLimitExtension

`src/builtins/rate_limit.rs`

基于滑动窗口的 tool call 频率限制。

```rust
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct RateLimitExtension {
    max_calls_per_minute: u64,
    call_times: Mutex<Vec<Instant>>,
}

impl RateLimitExtension {
    fn new(max_calls_per_minute: u64) -> Self {
        Self {
            max_calls_per_minute,
            call_times: Mutex::new(Vec::new()),
        }
    }
}

impl Extension for RateLimitExtension {
    fn name(&self) -> &str { "rate-limit" }

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        let mut times = self.call_times.lock().expect("rate-limit mutex poisoned");
        let now = Instant::now();

        // Prune entries older than 60s
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

### 9.3 ToolGuardExtension

`src/builtins/tool_guard.rs`

基于工具名的访问控制。

```rust
struct ToolGuardExtension {
    allowed_tools: Vec<String>,   // if non-empty, only these tools are allowed
    denied_tools: Vec<String>,    // these tools are always denied
}

impl ToolGuardExtension {
    fn new(allowed_tools: Vec<String>, denied_tools: Vec<String>) -> Self {
        Self { allowed_tools, denied_tools }
    }
}

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

**允许/拒绝规则：**
1. 如果 `ctx.tool_name` 在 `denied_tools` 中 → Block（即使也在 `allowed_tools` 中）
2. 如果 `allowed_tools` 非空且 `ctx.tool_name` 不在其中 → Block
3. 否则 → Continue

---

## 10. 与 agent-core 的接口契约

extensions crate 实现 `agent_core::hook_dispatcher::HookDispatcher` trait：

```rust
// Defined in agent_core::hook_dispatcher
#[async_trait]
pub trait HookDispatcher: Send + Sync {
    // Blocking (with mutation)
    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation);
    async fn on_before_compact(&self, ctx: &CompactCtx) -> CompactDecision;
    // Chain
    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation;
    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation;
    async fn on_before_agent_start(&self, ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation;
    async fn on_before_provider_request(&self, ctx: &ProviderRequestCtx) -> ProviderRequestMutation;
    async fn on_after_provider_response(&self, ctx: &ProviderResponseCtx) -> ProviderResponseMutation;
    // Observational
    async fn on_turn_end(&self, ctx: &TurnEndCtx);
    async fn on_agent_end(&self, ctx: &AgentEndCtx);
    async fn on_session_start(&self, ctx: &SessionCtx);
    async fn on_tool_execution_start(&self, ctx: &ToolExecutionStartCtx);
    async fn on_tool_execution_update(&self, ctx: &ToolExecutionUpdateCtx);
    async fn on_tool_execution_end(&self, ctx: &ToolExecutionEndCtx);
    async fn on_compact_end(&self, ctx: &CompactEndCtx);
}
```

| agent-core 类型 | extensions 用途 |
|---|---|
| `HookDispatcher` trait | 由 `HookRouter` 实现（14 个方法） |
| `HookDecision { Continue, Block { reason } }` | 阻断型 hook (on_tool_call) decision 部分 |
| `ToolCallMutation { input }` | 阻断型 hook (on_tool_call) mutation 部分，支持参数修改回传 |
| `CompactDecision { Continue, Block, Replace }` | 阻断型 hook (on_before_compact) 返回值 |
| `ToolResultMutation`, `ContextMutation` | 链式 hook 返回值，链式合并 |
| `BeforeAgentStartMutation`, `ProviderRequestMutation`, `ProviderResponseMutation` | 链式 hook 返回值，链式合并 |
| `ToolCallCtx`, `ToolResultCtx`, `ContextCtx` | 传给链式/阻断 hook |
| `BeforeAgentStartCtx`, `ProviderRequestCtx`, `ProviderResponseCtx` | 传给链式 hook |
| `CompactCtx` | 传给 on_before_compact（阻断型） |
| `TurnEndCtx`, `AgentEndCtx`, `SessionCtx` | EventBus 广播（观测型） |
| `ToolExecutionStartCtx`, `ToolExecutionUpdateCtx`, `ToolExecutionEndCtx` | EventBus 广播（观测型） |
| `CompactEndCtx` | EventBus 广播（观测型） |
| `AgentTool` trait | `ExtensionTool` 实现此 trait，将 Extension 工具包装为可执行对象 |
| `AgentToolResult` | `execute_tool()` 返回值 |
| `AgentError` | `execute_tool()` 错误返回值 |

---

## 11. 测试计划

### 11.1 HookRouter 测试 (`tests/hook_router_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_blocking_first_block_wins` | ext1 returns Block → HookRouter returns Block; ext2 never called |
| `test_blocking_all_continue` | two extensions both Continue → HookRouter returns Continue |
| `test_blocking_timeout_continue` | extension hangs → 500ms timeout → treated as Continue (with default mutation), next extension called |
| `test_tool_call_mutation_chain` | ext1 sanitizes input, ext2 Continue → final mutation contains ext1's sanitized input |
| `test_tool_call_mutation_block_retained` | ext1 sanitizes input, ext2 Block → final mutation still contains ext1's sanitized input |
| `test_tool_call_mutation_multi_handler` | ext1 modifies input, ext2 further modifies based on ext1's changes → final mutation has ext2's version |
| `test_tool_call_mutation_timeout_skips` | ext1 times out → skipped (default mutation), ext2 modifies → final mutation has ext2's version |
| `test_chain_merge_accumulates` | ext1 changes content, ext2 changes details → final mutation has both |
| `test_chain_merge_timeout_skips` | ext1 times out → skipped, ext2's mutation applied |
| `test_chain_merge_partial_mutation` | ext returns only content (no details) → details unchanged |
| `test_chain_merge_ctx_propagation` | ext1 mutation visible to ext2's ctx |
| `test_observation_broadcast_to_all` | EventBus emit → all subscriber actors receive |
| `test_observation_does_not_block_caller` | HookRouter::on_turn_end returns immediately (<1ms) |
| `test_before_agent_start_chain_merge` | ext1 sets system_prompt, ext2 sets messages → accumulated |
| `test_before_agent_start_timeout_skips` | ext1 times out → skipped, ext2's mutation applied |
| `test_before_provider_request_modifies_options` | ext changes max_tokens/temperature → mutation accumulated |
| `test_on_before_compact_first_block_wins` | ext1 returns Block → HookRouter returns Block; ext2 never called |
| `test_on_before_compact_replace` | ext returns Replace { result } → HookRouter returns Replace |
| `test_on_before_compact_all_continue` | all extensions Continue → HookRouter returns Continue |
| `test_tool_execution_events_broadcast` | EventBus emit ToolExecutionStart → all subscriber actors receive |
| `test_compact_end_broadcast` | EventBus emit CompactEnd → all subscriber actors receive |

### 11.2 ExtensionActor 测试 (`tests/extension_actor_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_actor_startup_shutdown` | spawn → send Shutdown → task exits cleanly |
| `test_actor_on_tool_call_reply` | send OnToolCall command → receive correct (HookDecision, ToolCallMutation) via oneshot |
| `test_actor_panic_isolation` | extension panics in on_tool_call → actor survives, next command processed |
| `test_actor_panic_returns_continue` | panic in blocking hook → caller receives Continue |
| `test_actor_oneshot_timeout` | actor doesn't reply → caller gets AskError::Timeout |
| `test_actor_eventbus_receive` | EventBus emit TurnEnd → actor receives and invokes extension.on_turn_end |
| `test_actor_observation_timeout` | extension.on_turn_end() hangs → 100ms timeout → actor continues processing |
| `test_actor_shutdown_drops_handle` | actor gone → ask() returns AskError::ActorGone |

### 11.3 Builtin Extension 测试

| 测试 | 验证点 |
|---|---|
| `test_audit_emits_traces_for_tool_calls` | on_tool_call + on_tool_result → tracing spans emitted |
| `test_audit_never_blocks` | always returns Continue |
| `test_audit_never_mutates` | always returns default mutation |
| `test_rate_limit_allows_within_budget` | 3 calls within 60s → all Continue |
| `test_rate_limit_blocks_when_exceeded` | exceed max → returns Block with reason |
| `test_rate_limit_window_rotates` | wait 60s → counter resets → new calls allowed |
| `test_tool_guard_blocks_denied` | ctx.tool_name in denied → Block |
| `test_tool_guard_allows_allowed` | ctx.tool_name in allowed → Continue |
| `test_tool_guard_blocks_unknown_when_allowlist_set` | allowed non-empty + tool not in list → Block |
| `test_tool_guard_denied_overrides_allowed` | tool in both lists → denied wins → Block |

---

## 12. 关键设计决策

| 决策 | 理由 |
|---|---|
| 每个 Extension 独立 tokio task | ADR-004 要求 session 级隔离。独立 task 隔离 panic、独立超时、独立 shutdown |
| `mpsc` 用于 mailbox，`broadcast` 用于 EventBus | 阻塞/链式需要 request-reply → `mpsc + oneshot`。观测型需要一对多 → `broadcast` |
| oneshot 超时 500ms（阻塞/链式），100ms（观测） | ADR-003 规定。阻塞超时默认 Continue，链式超时跳过 handler |
| Panic 捕获 → 默认值 | AGENTS.md 要求 Extension panic 不得传播到 agent loop |
| 扩展顺序决定优先级 | 阻断型 first-block-wins 依赖顺序；链式合并顺序影响最终结果。注册顺序即遍历顺序 |
| 工具去重 first-registration-wins | 避免同名工具冲突。pi.dev 采用相同策略 |
| `std::sync::Mutex` 用于 RateLimitExtension 状态 | 临界区极短（`retain` + `push`），不阻塞 async executor。仅限 builtin 内部使用，不跨 session 共享 |
| 观测事件在 `select!` 循环中处理，不额外 spawn | 减少 tokio task 数量。每个 ExtensionActor 仅一个 task。观测 hook 有 100ms timeout，不会阻塞 mailbox 命令处理 |
| Shutdown 协议 | `ExtensionHandle` 持有 `mpsc::Sender`。Drop ExtensionHandle（或显式 close channel）使 actor 的 `mailbox.recv()` 返回 None → actor loop break。JoinHandles 用于 await actor 退出 |
| 测试结构 | 单元测试使用内联 `#[cfg(test)] mod tests`，集成测试放在 `tests/` 目录。内置 extension 测试使用内联模块 |
| Extension 工具同步 pi.dev 的定义+execute 模型 | `Extension::tools()` 提供 `ToolDef`，`Extension::execute_tool()` 提供执行逻辑。经 `ExtensionTool` wrapper 转为 `AgentTool`，由组装层与原生工具合并后注入 `SessionActor` |
| `execute_tool` 无框架超时 | 工具执行可能长达数分钟（如 bash 命令）。通过 `tokio::spawn` 在 Actor 中异步执行，不阻塞 mailbox。若 ExtensionActor 终止则 `RecvError` 映射为 `ToolExecutionFailed` |
| v0.1 不支持 Extension 工具进度回调 | `AgentTool::execute` 的 `on_progress` 在 Extension 路径中为 `None`。`on_tool_execution_update` 观测型 hook 不受影响（仍由 ToolExecutor 发出） |
| `on_tool_call` 支持 mutation 回传 | 阻断型 hook 通常不修改状态，但 `on_tool_call` 需要支持参数清洗、注入等场景。返回 `(HookDecision, ToolCallMutation)` 保持 first-block-wins 语义的同时允许 input 链式修改 |
| 工具合并发生在组装层 | `ExtensionManager::collect_agent_tools()` 在 `spawn_all()` 之后调用，使用返回的 `ExtensionHandle` 构造 `ExtensionTool`。合并由调用方执行（非 SessionActor），同名时扩展工具覆盖原生 |
