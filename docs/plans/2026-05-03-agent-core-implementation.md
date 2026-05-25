---

> **⚠️ DEPRECATED — Architecture Changed (v0.1.x)**
>
> This document was written for the **Extension Actor + EventBus** architecture, which has been **removed** in v0.1.x.
> The "extensions" crate, ExtensionActor, HookRouter, and EventBus no longer exist.
> Built-in strategies (audit, path_guard, tool_guard, token_budget) are now inlined in agent-core::hook::DefaultHookDispatcher.
> Hook calls are direct function calls (no Actor, no EventBus, no timeout boundaries).
> See [AGENTS.md](../../AGENTS.md) (ADR-002, ADR-003) for the current architecture.

---

# agent-core Implementation Plan

> **优先级: P0（阻塞级）** — 本 crate 的 Phase 0 新类型是 extensions crate 的前置依赖。必须先于 extensions 启动。
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Evolve `crates/agent-core/` from its current skeleton to the full spec defined in `docs/specs/2026-05-02-agent-core.md`, including: AgentEvent system, SessionEntry/compaction, expanded HookDispatcher, error recovery state machine, orphan tool call resolution, and LLM retry logic.

**Architecture:** Bottom-up: types/contexts → events → hook dispatcher → tool executor → agent loop → compaction → session actor → error recovery. Each layer is testable independently.

**Tech Stack:** Rust 2024 edition, tokio, async-trait, thiserror, tracing, futures, tokio-util, uuid, serde_json.

**Spec Reference:** `docs/specs/2026-05-02-agent-core.md`

**阻塞依赖:**
- **被 extensions 阻塞**: Phase 0 完成后，extensions 才能启动 Phase 1-4
- **阻塞 extensions**: Phase 0 交付 8 个新 Ctx 类型 + 5 个新 Mutation 类型（见下方 Phase 0）

**开发顺序（联合视图）:**
```
Week 1: Phase 0 (P0) → Phase 1-2 (P0)
Week 1-2: Phase 3-4 (P0)  [与 extensions Phase 1-4 并行]
Week 2-3: Phase 5-7 (P0)  [与 extensions Phase 5 并行]
Week 3-4: Phase 8-11 (P0/P1)
```

---

## Current State

The codebase already has:
- `AgentLoop` with basic inner turn loop
- `SessionActor` with prompt/steer/follow_up/abort
- `HookDispatcher` trait (3 methods: on_tool_call, on_tool_result, on_context)
- `ToolExecutor` with blocking + chain hooks
- Basic error types and context types
- 10 passing tests

**Missing:** AgentEvent system, SessionEntry/compaction, 4 new HookDispatcher methods, AgentLoopConfig, orphan resolution, LLM retry, CompactionActor, FileOperationExtractor, RecoveryStateMachine, ProviderStreamOptions, complete/continue_ methods.

---

## File Map

### New Files
| File | Purpose |
|---|---|
| `src/events.rs` | AgentEvent enum, AgentEventListener trait |
| `src/session_entry.rs` | SessionEntry enum, CompactionDetails, SessionContextBuilder |
| `src/compaction.rs` | CompactionActor with cut-point algorithm and LLM summary |
| `src/file_ops.rs` | FileOperationExtractor trait + DefaultFileOperationExtractor |
| `src/error_recovery.rs` | RecoveryStateMachine, RecoveryAction, is_session_retryable |
| `src/provider_opts.rs` | ProviderStreamOptions (safe subset of StreamOptions) |

### Modified Files
| File | Changes |
|---|---|
| `src/context.rs` | Add 6 new ctx structs: BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx, CompactCtx, ToolExecutionStartCtx, ToolExecutionUpdateCtx, ToolExecutionEndCtx, CompactEndCtx |
| `src/mutations.rs` | Add 4 new types: BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation, CompactDecision, ToolCallMutation |
| `src/hook_dispatcher.rs` | Add 5 methods: on_before_agent_start, on_before_provider_request, on_after_provider_response, on_before_compact, on_tool_execution_* (observational) |
| `src/loop.rs` | Refactor to AgentLoopConfig + outer/inner loop, add retry, orphan resolution, event emission |
| `src/session.rs` | Add entries, event queue, auto-compaction, error recovery, complete/continue_ |
| `src/error.rs` | Add CompactionFailed variant |
| `src/lib.rs` | Export new modules |
| `Cargo.toml` | Add `uuid` dependency |

---

## Phase 0: Foundation Types (P0 — 阻塞级，必须最先完成)

> **⚠️ 阻塞声明**: 本 Phase 交付的 8 个 Ctx 类型和 5 个 Mutation 类型是 extensions crate 的前置依赖。extensions 在以下类型可用前无法启动：
> - `CompactCtx`, `BeforeAgentStartCtx`, `ProviderRequestCtx`, `ProviderResponseCtx`
> - `ToolExecutionStartCtx`, `ToolExecutionUpdateCtx`, `ToolExecutionEndCtx`, `CompactEndCtx`
> - `CompactDecision`, `BeforeAgentStartMutation`, `ProviderRequestMutation`, `ProviderResponseMutation`, `ToolCallMutation`

### Task 0.1: Add uuid dependency (P0)

**Files:**
- Modify: `crates/agent-core/Cargo.toml`

**Steps:**

- [ ] **Step 1: Add uuid to dependencies**

```toml
[dependencies]
ai-provider = { path = "../ai-provider" }
tokio = { workspace = true }
async-trait = { workspace = true }
thiserror = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
futures = { workspace = true }
tokio-util = { workspace = true }
uuid = { version = "1", features = ["v4"] }
```

- [ ] **Step 2: Verify cargo check passes**

Run: `cargo check --package agent-core`
Expected: PASS (no new code yet)

---

### Task 0.2: New Context Types (P0)

**Files:**
- Modify: `crates/agent-core/src/context.rs`

**Steps:**

- [ ] **Step 1: Add new context structs**

Append to `src/context.rs`:

```rust
/// Context passed to Extension::on_before_agent_start
#[derive(Debug, Clone)]
pub struct BeforeAgentStartCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub system_prompt: Option<String>,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<serde_json::Value>,
    pub model: String,
}

/// Context passed to Extension::on_before_provider_request
#[derive(Debug, Clone)]
pub struct ProviderRequestCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub model: String,
    pub turn_index: u64,
    pub system_prompt: Option<String>,
    pub messages: Vec<AgentMessage>,
    pub tools: Option<Vec<llm_client::ToolDef>>,
    pub options: crate::provider_opts::ProviderStreamOptions,
}

/// Context passed to Extension::on_after_provider_response
#[derive(Debug, Clone)]
pub struct ProviderResponseCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub model: String,
    pub turn_index: u64,
    pub attempt: u32,
    pub messages_before: Vec<AgentMessage>,
    pub content: Vec<llm_client::Content>,
    pub stop_reason: llm_client::StopReason,
}

/// Context passed to Extension::on_before_compact
#[derive(Debug, Clone)]
pub struct CompactCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub preparation: crate::compaction::CompactionPreparation,
    pub entries: Vec<crate::session_entry::SessionEntry>,
    pub reason: CompactReason,
}

#[derive(Debug, Clone)]
pub enum CompactReason {
    Overflow,
    Threshold,
    Manual,
}

// Note: ToolExecutionStartCtx/ToolExecutionUpdateCtx/ToolExecutionEndCtx and CompactEndCtx
// are not needed in agent-core's HookDispatcher — these events are emitted via AgentEvent
// and consumed by AgentEventListener implementations (e.g., in api-gateway or tenant crates).
// Extension-specific observational hooks for these events would be defined in the extensions crate.
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --package agent-core`
Expected: PASS (structs compile)

---

### Task 0.3: New Mutation Types (P0)

**Files:**
- Modify: `crates/agent-core/src/mutations.rs`

**Steps:**

- [ ] **Step 1: Add new mutation types**

Append to `src/mutations.rs`:

```rust
/// Mutation returned by on_before_agent_start chain hook
#[derive(Debug, Clone, Default)]
pub struct BeforeAgentStartMutation {
    pub system_prompt: Option<String>,
    pub messages: Option<Vec<AgentMessage>>,
}

/// Mutation returned by on_before_provider_request chain hook
#[derive(Debug, Clone, Default)]
pub struct ProviderRequestMutation {
    pub system_prompt: Option<Option<String>>,
    pub messages: Option<Vec<AgentMessage>>,
    pub tools: Option<Option<Vec<llm_client::ToolDef>>>,
    pub options: Option<crate::provider_opts::ProviderStreamOptions>,
}

/// Mutation returned by on_after_provider_response chain hook
#[derive(Debug, Clone, Default)]
pub struct ProviderResponseMutation {
    pub content: Option<Vec<llm_client::Content>>,
    pub stop_reason: Option<llm_client::StopReason>,
}

/// Decision returned by on_before_compact blocking hook
#[derive(Debug, Clone)]
pub enum CompactDecision {
    Continue,
    Block { reason: String },
    Replace { result: crate::compaction::CompactionResult },
}

/// Mutation returned by on_tool_call blocking hook (with input mutation)
#[derive(Debug, Clone, Default)]
pub struct ToolCallMutation {
    pub input: Option<serde_json::Value>,
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --package agent-core`
Expected: PASS

---

### Task 0.4: ProviderStreamOptions (P0)

**Files:**
- Create: `crates/agent-core/src/provider_opts.rs`

**Steps:**

- [ ] **Step 1: Create ProviderStreamOptions**

```rust
use std::time::Duration;

/// Safe subset of StreamOptions for hook mutations.
/// Excludes callbacks and secrets.
#[derive(Debug, Clone, Default)]
pub struct ProviderStreamOptions {
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub reasoning: Option<llm_client::ReasoningLevel>,
    pub max_retries: Option<u32>,
    pub timeout: Option<Duration>,
}

impl ProviderStreamOptions {
    pub fn from_options(options: &llm_client::StreamOptions) -> Self {
        Self {
            max_tokens: options.max_tokens,
            temperature: options.temperature,
            top_p: options.top_p,
            reasoning: options.reasoning.clone(),
            max_retries: options.max_retries,
            timeout: options.timeout,
        }
    }
}
```

- [ ] **Step 2: Add to lib.rs**

Modify `src/lib.rs` to add `pub mod provider_opts;`

- [ ] **Step 3: Verify compilation**

Run: `cargo check --package agent-core`
Expected: PASS

---

### Task 0.5: Update HookDispatcher trait (P0)

**Files:**
- Modify: `crates/agent-core/src/hook_dispatcher.rs`

**Steps:**

- [ ] **Step 1: Expand HookDispatcher with new methods**

Replace `src/hook_dispatcher.rs` with:

```rust
use async_trait::async_trait;

use crate::context::*;
use crate::mutations::*;

/// Dependency-inversion boundary for extension hook dispatch.
///
/// Blocking hooks (`on_tool_call`, `on_before_compact`) follow first-block-wins semantics.
/// Chaining hooks (`on_tool_result`, `on_context`, `on_before_agent_start`, 
/// `on_before_provider_request`, `on_after_provider_response`) chain-merge mutations.
/// Observational hooks (`on_turn_end`, `on_agent_end`, `on_session_start`, 
/// `on_tool_execution_*`, `on_compact_end`) are fire-and-forget.
#[async_trait]
pub trait HookDispatcher: Send + Sync {
    /// Blocking hook — first-block-wins + input mutation chain
    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        (HookDecision::Continue, ToolCallMutation::default())
    }

    /// Blocking hook — first-block-wins
    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Continue
    }

    /// Chaining hook — each handler sees previous mutations
    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        ToolResultMutation::default()
    }

    /// Chaining hook — each handler transforms context messages
    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
        ContextMutation::default()
    }

    /// Chaining hook — before agent loop starts
    async fn on_before_agent_start(&self, _ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
        BeforeAgentStartMutation::default()
    }

    /// Chaining hook — before provider.stream() call
    async fn on_before_provider_request(&self, _ctx: &ProviderRequestCtx) -> ProviderRequestMutation {
        ProviderRequestMutation::default()
    }

    /// Chaining hook — after stream consumption, before tool extraction
    async fn on_after_provider_response(&self, _ctx: &ProviderResponseCtx) -> ProviderResponseMutation {
        ProviderResponseMutation::default()
    }

    /// Observational hook — fire-and-forget
    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {}

    /// Observational hook — fire-and-forget
    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {}

    /// Observational hook — fire-and-forget
    async fn on_session_start(&self, _ctx: &SessionCtx) {}
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --package agent-core`
Expected: PASS

---

## Phase 1: Events System (P0)

### Task 1.1: AgentEvent and AgentEventListener (P0)

**Files:**
- Create: `crates/agent-core/src/events.rs`

**Steps:**

- [ ] **Step 1: Define AgentEvent enum**

```rust
use crate::types::AgentMessage;
use crate::error::AgentError;
use crate::context::CompactReason;
use llm_client::ToolResultMessage;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd { messages: Vec<AgentMessage> },
    TurnStart { turn_index: u64 },
    TurnEnd { turn_index: u64, messages: Vec<AgentMessage> },
    MessageStart { message_index: u64 },
    MessageUpdate { message_index: u64, content_delta: String },
    MessageEnd { message: AgentMessage },
    ToolExecutionStart { tool_call_id: String, tool_name: String },
    ToolExecutionUpdate { tool_call_id: String, content: String },
    ToolExecutionEnd { tool_call_id: String, result: ToolResultMessage },
    CompactionStart { reason: CompactReason },
    CompactionEnd {
        reason: CompactReason,
        result: Option<crate::compaction::CompactionResult>,
        aborted: bool,
        will_retry: bool,
        error_message: Option<String>,
    },
    AutoRetryStart { attempt: u32, max_attempts: u32, delay_ms: u64 },
    AutoRetryEnd { success: bool, error: Option<String> },
    Error { error: AgentError },
}
```

- [ ] **Step 2: Define AgentEventListener trait**

```rust
use async_trait::async_trait;

#[async_trait]
pub trait AgentEventListener: Send + Sync {
    async fn on_event(&self, event: &AgentEvent);
}
```

- [ ] **Step 3: Add to lib.rs**

Add `pub mod events;` and `pub use events::{AgentEvent, AgentEventListener};`

- [ ] **Step 4: Verify compilation**

Run: `cargo check --package agent-core`
Expected: PASS

---

## Phase 2: SessionEntry and Context Builder (P0)

### Task 2.1: SessionEntry types (P0)

**Files:**
- Create: `crates/agent-core/src/session_entry.rs`

**Steps:**

- [ ] **Step 1: Define SessionEntry and related types**

```rust
use uuid::Uuid;
use std::time::SystemTime;
use crate::types::AgentMessage;

#[derive(Debug, Clone)]
pub enum SessionEntry {
    Message {
        id: Uuid,
        message: AgentMessage,
    },
    Compaction {
        id: Uuid,
        summary: String,
        first_kept_entry_id: Uuid,
        tokens_before: usize,
        details: Option<CompactionDetails>,
        from_extension: bool,
        timestamp: SystemTime,
    },
}

impl SessionEntry {
    pub fn id(&self) -> Option<Uuid> {
        match self {
            SessionEntry::Message { id, .. } => Some(*id),
            SessionEntry::Compaction { id, .. } => Some(*id),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompactionDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

pub struct SessionContextBuilder;

impl SessionContextBuilder {
    pub fn build_context(entries: &[SessionEntry]) -> Vec<AgentMessage> {
        let last_compaction_idx = entries
            .iter()
            .rposition(|e| matches!(e, SessionEntry::Compaction { .. }));
        let start_idx = last_compaction_idx.map(|i| i + 1).unwrap_or(0);

        let mut messages = Vec::new();

        if let Some(SessionEntry::Compaction { summary, .. }) =
            last_compaction_idx.map(|i| &entries[i])
        {
            messages.push(AgentMessage::User(llm_client::UserMessage {
                content: vec![llm_client::Content::Text {
                    text: format!("[Context Summary]\n{}", summary),
                    text_signature: None,
                }],
                timestamp: SystemTime::now(),
            }));
        }

        for entry in &entries[start_idx..] {
            if let SessionEntry::Message { message: msg, .. } = entry {
                if let AgentMessage::Assistant(assistant) = msg {
                    if assistant.stop_reason == llm_client::StopReason::Error {
                        continue;
                    }
                }
                messages.push(msg.clone());
            }
        }

        messages
    }
}
```

- [ ] **Step 2: Add to lib.rs**

Add `pub mod session_entry;` and exports.

- [ ] **Step 3: Write test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::{Content, UserMessage};

    #[test]
    fn test_build_context_skips_before_compaction() {
        let entries = vec![
            SessionEntry::Message {
                id: Uuid::new_v4(),
                message: AgentMessage::User(UserMessage {
                    content: vec![Content::Text { text: "old".to_string(), text_signature: None }],
                    timestamp: SystemTime::now(),
                }),
            },
            SessionEntry::Compaction {
                id: Uuid::new_v4(),
                summary: "summary".to_string(),
                first_kept_entry_id: Uuid::new_v4(),
                tokens_before: 100,
                details: None,
                from_extension: false,
                timestamp: SystemTime::now(),
            },
            SessionEntry::Message {
                id: Uuid::new_v4(),
                message: AgentMessage::User(UserMessage {
                    content: vec![Content::Text { text: "new".to_string(), text_signature: None }],
                    timestamp: SystemTime::now(),
                }),
            },
        ];

        let context = SessionContextBuilder::build_context(&entries);
        assert_eq!(context.len(), 2); // summary + new message
        match &context[1] {
            AgentMessage::User(u) => {
                assert_eq!(u.content[0].as_text().unwrap(), "new");
            }
            _ => panic!("expected user message"),
        }
    }
}
```

- [ ] **Step 4: Verify test passes**

Run: `cargo test --package agent-core session_entry`
Expected: PASS

---

## Phase 3: Error Recovery (P0)

### Task 3.1: RecoveryStateMachine (P0)

**Files:**
- Create: `crates/agent-core/src/error_recovery.rs`

**Steps:**

- [ ] **Step 1: Define RecoveryAction and RecoveryStateMachine**

```rust
use llm_client::{AssistantMessage, StopReason};

pub enum RecoveryAction {
    Continue,
    RetryAfterBackoff { delay_ms: u64 },
    RetryAfterCompaction { reason: crate::context::CompactReason },
    Abort { reason: String },
}

pub struct RecoveryStateMachine {
    overflow_attempted: bool,
    retry_count: u32,
    max_retries: u32,
}

impl RecoveryStateMachine {
    pub fn new(max_retries: u32) -> Self {
        Self {
            overflow_attempted: false,
            retry_count: 0,
            max_retries,
        }
    }

    pub fn evaluate(&mut self, msg: &AssistantMessage) -> RecoveryAction {
        if is_context_overflow(msg) {
            if self.overflow_attempted {
                return RecoveryAction::Abort {
                    reason: "Context overflow recovery failed after compact-and-retry".into(),
                };
            }
            self.overflow_attempted = true;
            return RecoveryAction::RetryAfterCompaction {
                reason: crate::context::CompactReason::Overflow,
            };
        }

        if is_session_retryable(msg) {
            self.retry_count += 1;
            if self.retry_count > self.max_retries {
                self.retry_count = 0;
                return RecoveryAction::Abort {
                    reason: "Max retry attempts exceeded".into(),
                };
            }
            let delay_ms = 100 * 2_u64.pow(self.retry_count - 1);
            return RecoveryAction::RetryAfterBackoff { delay_ms };
        }

        RecoveryAction::Continue
    }

    pub fn mark_success(&mut self) {
        self.retry_count = 0;
    }

    pub fn reset(&mut self) {
        self.retry_count = 0;
        self.overflow_attempted = false;
    }
}

fn is_context_overflow(msg: &AssistantMessage) -> bool {
    msg.stop_reason == StopReason::Error
        && msg.error_message.as_ref().map_or(false, |e| {
            let lower = e.to_lowercase();
            lower.contains("context length") || lower.contains("token limit")
        })
}

const RETRYABLE_PATTERNS: &[&str] = &[
    "overloaded", "rate limit", "429", "timeout", "network error",
    "service unavailable", "fetch failed", "terminated",
    "500", "502", "503", "504",
];

fn is_session_retryable(msg: &AssistantMessage) -> bool {
    msg.stop_reason == StopReason::Error
        && msg.error_message.as_ref().map_or(false, |e| {
            let lower = e.to_lowercase();
            RETRYABLE_PATTERNS.iter().any(|p| lower.contains(p))
        })
        && !is_context_overflow(msg)
}
```

- [ ] **Step 2: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::{AssistantMessage, Api, Usage};
    use std::time::SystemTime;

    fn make_assistant(stop_reason: StopReason, error_message: Option<String>) -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            provider: "test".to_string(),
            model: "test".to_string(),
            api: Api { provider: "test".to_string(), model: "test".to_string() },
            usage: Usage { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: None, cache_read_input_tokens: None, total_tokens: 0 },
            stop_reason,
            response_id: None,
            error_message,
            timestamp: SystemTime::now(),
        }
    }

    #[test]
    fn test_overflow_first_time() {
        let mut recovery = RecoveryStateMachine::new(3);
        let msg = make_assistant(StopReason::Error, Some("context length exceeded".to_string()));
        match recovery.evaluate(&msg) {
            RecoveryAction::RetryAfterCompaction { .. } => {},
            other => panic!("expected RetryAfterCompaction, got {:?}", other),
        }
    }

    #[test]
    fn test_overflow_second_time_aborts() {
        let mut recovery = RecoveryStateMachine::new(3);
        let msg = make_assistant(StopReason::Error, Some("context length exceeded".to_string()));
        recovery.evaluate(&msg);
        match recovery.evaluate(&msg) {
            RecoveryAction::Abort { .. } => {},
            other => panic!("expected Abort, got {:?}", other),
        }
    }

    #[test]
    fn test_retryable_within_limit() {
        let mut recovery = RecoveryStateMachine::new(3);
        let msg = make_assistant(StopReason::Error, Some("rate limit exceeded".to_string()));
        match recovery.evaluate(&msg) {
            RecoveryAction::RetryAfterBackoff { delay_ms: 100 } => {},
            other => panic!("expected RetryAfterBackoff(100), got {:?}", other),
        }
    }

    #[test]
    fn test_retryable_exhausted() {
        let mut recovery = RecoveryStateMachine::new(1);
        let msg = make_assistant(StopReason::Error, Some("overloaded".to_string()));
        recovery.evaluate(&msg);
        match recovery.evaluate(&msg) {
            RecoveryAction::Abort { .. } => {},
            other => panic!("expected Abort, got {:?}", other),
        }
    }

    #[test]
    fn test_mark_success_preserves_overflow() {
        let mut recovery = RecoveryStateMachine::new(3);
        let msg = make_assistant(StopReason::Error, Some("context length exceeded".to_string()));
        recovery.evaluate(&msg);
        recovery.mark_success();
        match recovery.evaluate(&msg) {
            RecoveryAction::Abort { .. } => {},
            other => panic!("expected Abort after mark_success, got {:?}", other),
        }
    }
}
```

- [ ] **Step 3: Add to lib.rs**

Add `pub mod error_recovery;` and exports.

- [ ] **Step 4: Verify tests pass**

Run: `cargo test --package agent-core error_recovery`
Expected: 6 tests PASS

---

## Phase 4: File Operations Extractor (P0)

### Task 4.1: FileOperationExtractor (P0)

**Files:**
- Create: `crates/agent-core/src/file_ops.rs`

**Steps:**

- [ ] **Step 1: Define FileOperationExtractor trait and default impl**

```rust
use crate::types::AgentMessage;
use llm_client::Content;

#[derive(Debug, Default, Clone)]
pub struct FileOperations {
    pub read: Vec<String>,
    pub written: Vec<String>,
    pub edited: Vec<String>,
}

pub trait FileOperationExtractor: Send + Sync {
    fn extract(&self, messages: &[AgentMessage]) -> FileOperations;
}

pub struct DefaultFileOperationExtractor {
    read_tool_names: Vec<String>,
    write_tool_names: Vec<String>,
    edit_tool_names: Vec<String>,
    path_arg_name: String,
}

impl Default for DefaultFileOperationExtractor {
    fn default() -> Self {
        Self {
            read_tool_names: vec!["read".to_string()],
            write_tool_names: vec!["write".to_string()],
            edit_tool_names: vec!["edit".to_string()],
            path_arg_name: "path".to_string(),
        }
    }
}

impl FileOperationExtractor for DefaultFileOperationExtractor {
    fn extract(&self, messages: &[AgentMessage]) -> FileOperations {
        let mut ops = FileOperations::default();

        for msg in messages {
            if let AgentMessage::Assistant(assistant) = msg {
                for content in &assistant.content {
                    if let Content::ToolCall(tc) = content {
                        let path = tc.arguments
                            .get(&self.path_arg_name)
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());

                        if let Some(path) = path {
                            if self.read_tool_names.contains(&tc.name) {
                                ops.read.push(path);
                            } else if self.write_tool_names.contains(&tc.name) {
                                ops.written.push(path);
                            } else if self.edit_tool_names.contains(&tc.name) {
                                ops.edited.push(path);
                            }
                        }
                    }
                }
            }
        }

        ops.read.sort_unstable();
        ops.read.dedup();
        ops.written.sort_unstable();
        ops.written.dedup();
        ops.edited.sort_unstable();
        ops.edited.dedup();

        ops
    }
}
```

- [ ] **Step 2: Add to lib.rs**

Add `pub mod file_ops;` and exports.

- [ ] **Step 3: Verify compilation**

Run: `cargo check --package agent-core`
Expected: PASS

---

## Phase 5: CompactionActor (P0)

### Task 5.1: CompactionConfig, CompactionResult, CompactionPreparation (P0)

**Files:**
- Create: `crates/agent-core/src/compaction.rs`

**Steps:**

- [ ] **Step 1: Define config and result types**

```rust
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use llm_client::{LlmProvider, LlmContext, StreamOptions, Content};
use crate::types::AgentMessage;
use crate::session_entry::{SessionEntry, CompactionDetails};
use crate::file_ops::{FileOperationExtractor, FileOperations};
use crate::error::AgentError;

#[derive(Debug, Clone)]
pub struct CompactionConfig {
    pub enabled: bool,
    pub reserve_tokens: usize,
    pub keep_recent_tokens: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 16384,
            keep_recent_tokens: 20000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: uuid::Uuid,
    pub tokens_before: usize,
    pub details: Option<CompactionDetails>,
}

#[derive(Debug, Clone)]
pub struct CompactionPreparation {
    pub first_kept_entry_id: uuid::Uuid,
    pub messages_to_summarize: Vec<AgentMessage>,
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: usize,
    pub previous_summary: Option<String>,
    pub file_ops: FileOperations,
}

#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("already compacted")]
    AlreadyCompacted,
    #[error("llm error: {0}")]
    LlmError(String),
}
```

- [ ] **Step 2: Define CompactionActor structure**

```rust
pub struct CompactionActor {
    pub config: CompactionConfig,
    provider: Arc<dyn LlmProvider>,
    model: String,
    file_op_extractor: Arc<dyn FileOperationExtractor>,
}

impl CompactionActor {
    pub fn new(
        config: CompactionConfig,
        provider: Arc<dyn LlmProvider>,
        model: String,
        file_op_extractor: Arc<dyn FileOperationExtractor>,
    ) -> Self {
        Self {
            config,
            provider,
            model,
            file_op_extractor,
        }
    }
}
```

- [ ] **Step 3: Implement token estimation helpers**

```rust
fn estimate_tokens(message: &AgentMessage) -> usize {
    let chars = match message {
        AgentMessage::User(user) => {
            user.content.iter().map(|c| match c {
                Content::Text { text } => text.len(),
                Content::Image { .. } => 4800,
                _ => 0,
            }).sum()
        }
        AgentMessage::Assistant(assistant) => {
            assistant.content.iter().map(|c| match c {
                Content::Text { text } => text.len(),
                Content::Thinking { thinking, .. } => thinking.len(),
                Content::ToolCall(tc) => tc.name.len() + serde_json::to_string(&tc.arguments).unwrap_or_default().len(),
                Content::Image { .. } => 4800,
            }).sum()
        }
        AgentMessage::ToolResult(result) => {
            result.content.iter().map(|c| match c {
                Content::Text { text } => text.len(),
                Content::Image { .. } => 4800,
                _ => 0,
            }).sum()
        }
    };
    (chars as f64 / 4.0).ceil() as usize
}

fn estimate_context_tokens(entries: &[SessionEntry]) -> usize {
    let mut tokens = 0;
    let mut last_usage_tokens: Option<usize> = None;
    let mut last_usage_idx: Option<usize> = None;

    for (i, entry) in entries.iter().enumerate() {
        if let SessionEntry::Message { message: AgentMessage::Assistant(assistant), .. } = entry {
            if assistant.stop_reason != llm_client::StopReason::Aborted 
                && assistant.stop_reason != llm_client::StopReason::Error {
                last_usage_tokens = Some(assistant.usage.total_tokens as usize);
                last_usage_idx = Some(i);
            }
        }
    }

    if let Some(usage_tokens) = last_usage_tokens {
        tokens = usage_tokens;
        if let Some(idx) = last_usage_idx {
            for entry in &entries[idx + 1..] {
                if let SessionEntry::Message { message: msg, .. } = entry {
                    tokens += estimate_tokens(msg);
                }
            }
        }
    } else {
        for entry in entries {
            if let SessionEntry::Message { message: msg, .. } = entry {
                tokens += estimate_tokens(msg);
            }
        }
    }

    tokens
}
```

- [ ] **Step 4: Implement cut-point algorithm**

```rust
#[derive(Debug)]
struct CutPoint {
    first_kept_entry_index: usize,
    turn_start_index: Option<usize>,
    is_split_turn: bool,
}

fn find_cut_point(
    entries: &[SessionEntry],
    start_index: usize,
    end_index: usize,
    keep_recent_tokens: usize,
) -> CutPoint {
    let cut_points = find_valid_cut_points(entries, start_index, end_index);

    if cut_points.is_empty() {
        return CutPoint {
            first_kept_entry_index: start_index,
            turn_start_index: None,
            is_split_turn: false,
        };
    }

    let mut accumulated = 0;
    let mut cut_index = cut_points[0];

    for i in (start_index..end_index).rev() {
        if let SessionEntry::Message { message: msg, .. } = &entries[i] {
            accumulated += estimate_tokens(msg);
            if accumulated >= keep_recent_tokens {
                cut_index = cut_points.iter()
                    .find(|&&cp| cp >= i)
                    .copied()
                    .unwrap_or(cut_points[0]);
                break;
            }
        }
    }

    while cut_index > start_index {
        match &entries[cut_index - 1] {
            SessionEntry::Compaction { .. } => break,
            SessionEntry::Message { .. } => break,
            _ => cut_index -= 1,
        }
    }

    let is_user_msg = matches!(
        &entries[cut_index],
        SessionEntry::Message { message: AgentMessage::User(_), .. }
    );

    let turn_start_index = if is_user_msg {
        None
    } else {
        find_turn_start_index(entries, cut_index, start_index)
    };

    CutPoint {
        first_kept_entry_index: cut_index,
        turn_start_index,
        is_split_turn: !is_user_msg && turn_start_index.is_some(),
    }
}

fn find_valid_cut_points(entries: &[SessionEntry], start: usize, end: usize) -> Vec<usize> {
    let mut points = Vec::new();
    for i in start..end {
        match &entries[i] {
            SessionEntry::Message { message: msg, .. } => match msg {
                AgentMessage::User(_) | AgentMessage::Assistant(_) => points.push(i),
                AgentMessage::ToolResult(_) => {}
            },
            _ => {}
        }
    }
    points
}

fn find_turn_start_index(entries: &[SessionEntry], entry_index: usize, start: usize) -> Option<usize> {
    for i in (start..=entry_index).rev() {
        match &entries[i] {
            SessionEntry::Message { message: AgentMessage::User(_), .. } => return Some(i),
            _ => {}
        }
    }
    None
}
```

- [ ] **Step 5: Implement prepare and compact methods**

```rust
impl CompactionActor {
    pub fn prepare(&self, entries: &[SessionEntry]) -> Result<CompactionPreparation, CompactionError> {
        if let Some(SessionEntry::Compaction { .. }) = entries.last() {
            return Err(CompactionError::AlreadyCompacted);
        }

        let prev_compaction_idx = entries.iter().rposition(|e| matches!(e, SessionEntry::Compaction { .. }));
        let mut previous_summary = None;
        let mut boundary_start = 0;

        if let Some(idx) = prev_compaction_idx {
            if let SessionEntry::Compaction { summary, first_kept_entry_id, .. } = &entries[idx] {
                previous_summary = Some(summary.clone());
                boundary_start = entries.iter().position(|e| {
                    matches!(e, SessionEntry::Message { id, .. } if id == first_kept_entry_id)
                }).unwrap_or(idx + 1);
            }
        }

        let boundary_end = entries.len();
        let tokens_before = estimate_context_tokens(entries);

        let cut_point = find_cut_point(entries, boundary_start, boundary_end, self.config.keep_recent_tokens);

        let history_end = if cut_point.is_split_turn {
            cut_point.turn_start_index.unwrap_or(cut_point.first_kept_entry_index)
        } else {
            cut_point.first_kept_entry_index
        };

        let mut messages_to_summarize = Vec::new();
        for i in boundary_start..history_end {
            if let SessionEntry::Message { message: msg, .. } = &entries[i] {
                messages_to_summarize.push(msg.clone());
            }
        }

        let mut turn_prefix_messages = Vec::new();
        if cut_point.is_split_turn {
            for i in cut_point.turn_start_index.unwrap()..cut_point.first_kept_entry_index {
                if let SessionEntry::Message { message: msg, .. } = &entries[i] {
                    turn_prefix_messages.push(msg.clone());
                }
            }
        }

        let file_ops = self.file_op_extractor.extract(&messages_to_summarize);

        let first_kept_entry_id = entries[cut_point.first_kept_entry_index]
            .id()
            .unwrap_or_else(uuid::Uuid::new_v4);

        Ok(CompactionPreparation {
            first_kept_entry_id,
            messages_to_summarize,
            turn_prefix_messages,
            is_split_turn: cut_point.is_split_turn,
            tokens_before,
            previous_summary,
            file_ops,
        })
    }

    pub async fn compact(
        &self,
        entries: &[SessionEntry],
        signal: &CancellationToken,
    ) -> Result<CompactionResult, CompactionError> {
        let preparation = self.prepare(entries)?;
        let summary = self.generate_summary(&preparation, signal).await?;

        let details = if preparation.file_ops.read.is_empty() 
            && preparation.file_ops.written.is_empty() 
            && preparation.file_ops.edited.is_empty() {
            None
        } else {
            Some(CompactionDetails {
                read_files: preparation.file_ops.read,
                modified_files: preparation.file_ops.written.into_iter()
                    .chain(preparation.file_ops.edited.into_iter())
                    .collect(),
            })
        };

        Ok(CompactionResult {
            summary,
            first_kept_entry_id: preparation.first_kept_entry_id,
            tokens_before: preparation.tokens_before,
            details,
        })
    }
}
```

- [ ] **Step 6: Implement summary generation (stubs for now)**

```rust
impl CompactionActor {
    async fn generate_summary(
        &self,
        preparation: &CompactionPreparation,
        signal: &CancellationToken,
    ) -> Result<String, CompactionError> {
        let max_tokens = (self.config.reserve_tokens as f64 * 0.8) as usize;

        if preparation.is_split_turn && !preparation.turn_prefix_messages.is_empty() {
            let history_future = self.generate_history_summary(
                &preparation.messages_to_summarize,
                preparation.previous_summary.clone(),
                max_tokens,
                signal,
            );
            let prefix_future = self.generate_turn_prefix_summary(
                &preparation.turn_prefix_messages,
                (self.config.reserve_tokens as f64 * 0.5) as usize,
                signal,
            );
            let (history_result, prefix_result) = tokio::try_join!(history_future, prefix_future)?;
            Ok(format!(
                "{}\n\n---\n\n**Turn Context (split turn):**\n\n{}",
                history_result, prefix_result
            ))
        } else {
            self.generate_history_summary(
                &preparation.messages_to_summarize,
                preparation.previous_summary.clone(),
                max_tokens,
                signal,
            ).await
        }
    }

    async fn generate_history_summary(
        &self,
        messages: &[AgentMessage],
        previous_summary: Option<String>,
        max_tokens: usize,
        signal: &CancellationToken,
    ) -> Result<String, CompactionError> {
        let base_prompt = if previous_summary.is_some() {
            UPDATE_SUMMARIZATION_PROMPT
        } else {
            SUMMARIZATION_PROMPT
        };

        let conversation_text = serialize_messages(messages);
        let mut prompt_text = format!("<conversation>\n{}\n</conversation>\n\n", conversation_text);
        if let Some(prev) = previous_summary {
            prompt_text.push_str(&format!("<previous-summary>\n{}\n</previous-summary>\n\n", prev));
        }
        prompt_text.push_str(base_prompt);

        let llm_messages = vec![llm_client::Message::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text { text: prompt_text, text_signature: None }],
            timestamp: SystemTime::now(),
        })];

        let ctx = LlmContext {
            system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
            messages: llm_messages,
            tools: None,
        };

        let mut stream = self.provider.stream(
            &self.model,
            ctx,
            StreamOptions { max_tokens: Some(max_tokens as u32), ..Default::default() },
            signal.child_token(),
        ).await.map_err(|e| CompactionError::LlmError(e.to_string()))?;

        let mut summary_text = String::new();
        while let Some(event) = stream.next().await {
            match event {
                llm_client::AssistantMessageEvent::TextDelta { delta, .. } => {
                    summary_text.push_str(&delta);
                }
                llm_client::AssistantMessageEvent::Done { message, .. } => {
                    summary_text = message.content.iter()
                        .filter_map(|c| match c {
                            llm_client::Content::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    break;
                }
                llm_client::AssistantMessageEvent::Error { error, .. } => {
                    return Err(CompactionError::LlmError(error.to_string()));
                }
                _ => {}
            }
        }

        if summary_text.is_empty() {
            return Err(CompactionError::LlmError("Summary generation returned empty text".into()));
        }
        Ok(summary_text)
    }

    async fn generate_turn_prefix_summary(
        &self,
        messages: &[AgentMessage],
        max_tokens: usize,
        signal: &CancellationToken,
    ) -> Result<String, CompactionError> {
        let conversation_text = serialize_messages(messages);
        let prompt_text = format!(
            "<conversation>\n{}\n</conversation>\n\n{}",
            conversation_text, TURN_PREFIX_SUMMARIZATION_PROMPT
        );

        let llm_messages = vec![llm_client::Message::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text { text: prompt_text, text_signature: None }],
            timestamp: SystemTime::now(),
        })];

        let ctx = LlmContext {
            system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
            messages: llm_messages,
            tools: None,
        };

        let mut stream = self.provider.stream(
            &self.model,
            ctx,
            StreamOptions { max_tokens: Some(max_tokens as u32), ..Default::default() },
            signal.child_token(),
        ).await.map_err(|e| CompactionError::LlmError(e.to_string()))?;

        let mut summary_text = String::new();
        while let Some(event) = stream.next().await {
            match event {
                llm_client::AssistantMessageEvent::TextDelta { delta, .. } => {
                    summary_text.push_str(&delta);
                }
                llm_client::AssistantMessageEvent::Done { message, .. } => {
                    summary_text = message.content.iter()
                        .filter_map(|c| match c {
                            llm_client::Content::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    break;
                }
                llm_client::AssistantMessageEvent::Error { error, .. } => {
                    return Err(CompactionError::LlmError(error.to_string()));
                }
                _ => {}
            }
        }

        if summary_text.is_empty() {
            return Err(CompactionError::LlmError("Turn prefix summary returned empty text".into()));
        }
        Ok(summary_text)
    }
}

// Prompt templates
const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a conversation summarizer. ..."#;

const SUMMARIZATION_PROMPT: &str = r#"Summarize the conversation above into a structured format:
- Overview
- Progress (Done / In Progress)
- Key Decisions
- Current State
- Next Steps
- Important files and functions mentioned

Be concise but preserve exact file paths, function names, and error messages."#;

const UPDATE_SUMMARIZATION_PROMPT: &str = r#"The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it"#;

const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = r#"This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix. Be concise. Focus on what's needed to understand the kept suffix."#;

fn serialize_messages(messages: &[AgentMessage]) -> String {
    let mut output = String::new();
    for msg in messages {
        let (role, text) = match msg {
            AgentMessage::User(user) => ("User", extract_text(&user.content)),
            AgentMessage::Assistant(assistant) => ("Assistant", extract_text(&assistant.content)),
            AgentMessage::ToolResult(result) => ("Tool", extract_text(&result.content)),
        };
        output.push_str(&format!("[{}]: {}\n\n", role, text));
    }
    output
}

fn extract_text(content: &[llm_client::Content]) -> String {
    content.iter().filter_map(|c| match c {
        llm_client::Content::Text { text } => Some(text.as_str()),
        _ => None,
    }).collect::<Vec<_>>().join(" ")
}
}
```

- [ ] **Step 7: Add to lib.rs**

Add `pub mod compaction;` and exports.

- [ ] **Step 8: Write tests for cut-point logic**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::{Content, UserMessage, AssistantMessage, Api, Usage};
    use std::time::SystemTime;

    fn make_user(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: vec![Content::Text { text: text.to_string(), text_signature: None }],
            timestamp: SystemTime::now(),
        })
    }

    fn make_assistant(text: &str) -> AgentMessage {
        AgentMessage::Assistant(AssistantMessage {
            content: vec![Content::Text { text: text.to_string(), text_signature: None }],
            provider: "test".to_string(),
            model: "test".to_string(),
            api: Api { provider: "test".to_string(), model: "test".to_string() },
            usage: Usage { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: None, cache_read_input_tokens: None, total_tokens: 0 },
            stop_reason: llm_client::StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: SystemTime::now(),
        })
    }

    #[test]
    fn test_find_cut_point_basic() {
        let entries: Vec<SessionEntry> = (0..5)
            .map(|i| SessionEntry::Message {
                id: uuid::Uuid::new_v4(),
                message: if i % 2 == 0 { make_user("hi") } else { make_assistant("hello") },
            })
            .collect();

        let cut = find_cut_point(&entries, 0, entries.len(), 10);
        // Should keep some recent entries
        assert!(cut.first_kept_entry_index < entries.len());
    }

    #[test]
    fn test_valid_cut_points_skip_tool_results() {
        let entries = vec![
            SessionEntry::Message { id: uuid::Uuid::new_v4(), message: make_user("u1") },
            SessionEntry::Message { id: uuid::Uuid::new_v4(), message: make_assistant("a1") },
            SessionEntry::Message { 
                id: uuid::Uuid::new_v4(), 
                message: AgentMessage::ToolResult(llm_client::ToolResultMessage {
                    tool_call_id: "1".to_string(),
                    tool_name: "test".to_string(),
                    content: vec![],
                    details: None,
                    is_error: false,
                    timestamp: SystemTime::now(),
                })
            },
        ];

        let points = find_valid_cut_points(&entries, 0, entries.len());
        assert_eq!(points.len(), 2); // user and assistant, not tool result
    }
}
```

- [ ] **Step 9: Verify tests pass**

Run: `cargo test --package agent-core compaction`
Expected: PASS

---

## Phase 6: Tool Executor Update (P0)

### Task 6.1: Add ToolCallMutation support (P0)

**Files:**
- Modify: `crates/agent-core/src/tool.rs`

**Steps:**

- [ ] **Step 1: Update ToolExecutor to use ToolCallMutation**

Modify `execute_tool_call` to handle input mutation:

```rust
pub async fn execute_tool_call(
    &self,
    tool_call: &ToolCall,
    on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
) -> Result<ToolResultMsg, AgentError> {
    // Step 1: Dispatch on_tool_call (blocking hook with input mutation)
    let mut tool_call_ctx = ToolCallCtx {
        tenant_id: self.tenant_id.clone(),
        session_id: self.session_id.clone(),
        tool_name: tool_call.name.clone(),
        tool_call_id: tool_call.id.clone(),
        input: tool_call.arguments.clone(),
    };
    let (decision, mutation) = self.hook_dispatcher.on_tool_call(&tool_call_ctx).await;
    
    // Apply input mutation
    if let Some(input) = mutation.input {
        tool_call_ctx.input = input;
    }
    
    match decision {
        HookDecision::Block { reason } => {
            warn!(
                tenant_id = %self.tenant_id,
                session_id = %self.session_id,
                tool_name = %tool_call.name,
                reason = %reason,
                "tool call blocked by hook",
            );
            return Ok(ToolResultMsg {
                tool_call_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                content: vec![],
                details: Some(serde_json::json!({"blocked": true, "reason": reason})),
                is_error: true,
                timestamp: std::time::SystemTime::now(),
            });
        }
        HookDecision::Continue => {}
    }

    info!(
        tenant_id = %self.tenant_id,
        session_id = %self.session_id,
        tool_name = %tool_call.name,
        tool_call_id = %tool_call.id,
        "executing tool",
    );

    // Step 2: Execute with potentially mutated input
    let mut result = self.tool.execute(&tool_call.id, tool_call_ctx.input.clone(), on_progress).await?;

    // Step 3: Dispatch on_tool_result (chaining hook)
    let tool_result_ctx = ToolResultCtx {
        tenant_id: self.tenant_id.clone(),
        session_id: self.session_id.clone(),
        tool_name: tool_call.name.clone(),
        tool_call_id: tool_call.id.clone(),
        input: tool_call_ctx.input,
        content: result.content.clone(),
        details: result.details.clone(),
        is_error: result.is_error,
    };
    let mutation = self.hook_dispatcher.on_tool_result(&tool_result_ctx).await;

    // Apply mutations
    if let Some(content) = mutation.content {
        result.content = content;
    }
    if let Some(details) = mutation.details {
        result.details = Some(details);
    }
    if let Some(is_error) = mutation.is_error {
        result.is_error = is_error;
    }
    if let Some(terminate) = mutation.terminate {
        result.terminate = terminate;
    }

    let details = {
        let mut d = result.details.unwrap_or(serde_json::json!({}));
        if result.terminate {
            d["_terminate"] = serde_json::json!(true);
        }
        Some(d)
    };

    Ok(ToolResultMsg {
        tool_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        content: result.content,
        details,
        is_error: result.is_error,
        timestamp: std::time::SystemTime::now(),
    })
}
```

- [ ] **Step 2: Verify tests still pass**

Run: `cargo test --package agent-core tool`
Expected: PASS (existing tests)

---

## Phase 7: AgentLoop Refactor (P0 — 高风险，需仔细验证)

### Task 7.1: AgentLoopConfig and resolve_orphan_tool_calls (P0)

**Files:**
- Modify: `crates/agent-core/src/loop.rs`

**Steps:**

- [ ] **Step 1: Add AgentLoopConfig and orphan resolution**

Add at top of `loop.rs`:

```rust
use std::sync::{Arc, Mutex};

pub struct AgentLoopConfig {
    pub tenant_id: String,
    pub session_id: String,
    pub model: String,
    pub provider: Arc<dyn LlmProvider>,
    pub hook_dispatcher: Arc<dyn HookDispatcher>,
    pub tools: Vec<AgentToolRef>,
    pub system_prompt: Option<String>,
    pub stream_options: StreamOptions,
    pub max_retries: u32,
    pub steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    pub follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    pub event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync>,
}

fn resolve_orphan_tool_calls(messages: &mut Vec<AgentMessage>) {
    use std::collections::HashSet;

    let mut tool_call_ids: Vec<(usize, String)> = Vec::new();
    let mut resolved_ids: HashSet<String> = HashSet::new();

    for (i, msg) in messages.iter().enumerate() {
        match msg {
            AgentMessage::Assistant(a) => {
                for content in &a.content {
                    if let llm_client::Content::ToolCall(tc) = content {
                        tool_call_ids.push((i, tc.id.clone()));
                    }
                }
            }
            AgentMessage::ToolResult(tr) => {
                resolved_ids.insert(tr.tool_call_id.clone());
            }
            _ => {}
        }
    }

    let mut orphans: Vec<(usize, String, String)> = tool_call_ids
        .into_iter()
        .filter(|(_, id)| !resolved_ids.contains(id))
        .map(|(idx, id)| {
            let tool_name = match &messages[idx] {
                AgentMessage::Assistant(a) => a.content.iter().find_map(|c| match c {
                    llm_client::Content::ToolCall(tc) if tc.id == id => Some(tc.name.clone()),
                    _ => None,
                }),
                _ => None,
            }
            .unwrap_or_else(|| "unknown".to_string());
            (idx, id, tool_name)
        })
        .collect();

    orphans.sort_by(|a, b| b.0.cmp(&a.0));
    for (idx, id, tool_name) in orphans {
        let result_msg = AgentMessage::ToolResult(llm_client::ToolResultMessage {
            tool_call_id: id.clone(),
            tool_name,
            content: vec![],
            details: Some(serde_json::json!({
                "_orphan": true,
                "message": "tool call was not executed (context truncated or restored)"
            })),
            is_error: true,
            timestamp: std::time::SystemTime::now(),
        });
        messages.insert(idx + 1, result_msg);
    }
}
```

- [ ] **Step 2: Write test for orphan resolution**

```rust
#[cfg(test)]
mod orphan_tests {
    use super::*;
    use llm_client::{ToolCall, Content, AssistantMessage, Api, Usage};
    use std::time::SystemTime;

    #[test]
    fn test_resolve_orphan_tool_calls() {
        let mut messages = vec![
            AgentMessage::Assistant(AssistantMessage {
                content: vec![Content::ToolCall(ToolCall {
                    id: "call_1".to_string(),
                    name: "test_tool".to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                })],
                provider: "test".to_string(),
                model: "test".to_string(),
                api: Api { provider: "test".to_string(), model: "test".to_string() },
                usage: Usage { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: None, cache_read_input_tokens: None, total_tokens: 0 },
                stop_reason: llm_client::StopReason::ToolUse,
                response_id: None,
                error_message: None,
                timestamp: SystemTime::now(),
            }),
        ];

        resolve_orphan_tool_calls(&mut messages);
        assert_eq!(messages.len(), 2);
        match &messages[1] {
            AgentMessage::ToolResult(tr) => {
                assert_eq!(tr.tool_call_id, "call_1");
                assert!(tr.details.as_ref().unwrap()["_orphan"].as_bool().unwrap());
            }
            _ => panic!("expected tool result"),
        }
    }

    #[test]
    fn test_no_orphan_when_resolved() {
        let mut messages = vec![
            AgentMessage::Assistant(AssistantMessage {
                content: vec![Content::ToolCall(ToolCall {
                    id: "call_1".to_string(),
                    name: "test_tool".to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                })],
                provider: "test".to_string(),
                model: "test".to_string(),
                api: Api { provider: "test".to_string(), model: "test".to_string() },
                usage: Usage { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: None, cache_read_input_tokens: None, total_tokens: 0 },
                stop_reason: llm_client::StopReason::ToolUse,
                response_id: None,
                error_message: None,
                timestamp: SystemTime::now(),
            }),
            AgentMessage::ToolResult(llm_client::ToolResultMessage {
                tool_call_id: "call_1".to_string(),
                tool_name: "test_tool".to_string(),
                content: vec![],
                details: None,
                is_error: false,
                timestamp: SystemTime::now(),
            }),
        ];

        resolve_orphan_tool_calls(&mut messages);
        assert_eq!(messages.len(), 2);
    }
}
```

- [ ] **Step 3: Verify orphan tests pass**

Run: `cargo test --package agent-core orphan`
Expected: PASS

---

### Task 7.2: Refactor AgentLoop::run to spec (P0 — 最大变更量)

**Files:**
- Modify: `crates/agent-core/src/loop.rs`

**Steps:**

- [ ] **Step 1: Add helper functions and tool defs builders**

```rust
fn build_tool_defs(tools: &[AgentToolRef]) -> Option<Vec<llm_client::ToolDef>> {
    if tools.is_empty() {
        None
    } else {
        Some(
            tools
                .iter()
                .map(|t| llm_client::ToolDef {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: t.parameters(),
                })
                .collect(),
        )
    }
}

fn build_tool_value_defs(tools: &[AgentToolRef]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name(),
                "description": t.description(),
                "parameters": t.parameters(),
            })
        })
        .collect()
}

fn apply_provider_request_mutation(
    ctx: &mut LlmContext,
    opts: &mut StreamOptions,
    mutation: ProviderRequestMutation,
) {
    if let Some(sp) = mutation.system_prompt {
        ctx.system_prompt = sp;
    }
    if let Some(msgs) = mutation.messages {
        ctx.messages = msgs;
    }
    if let Some(tools) = mutation.tools {
        ctx.tools = tools;
    }
    if let Some(options) = mutation.options {
        if let Some(mt) = options.max_tokens { opts.max_tokens = Some(mt); }
        if let Some(temp) = options.temperature { opts.temperature = Some(temp); }
        if let Some(tp) = options.top_p { opts.top_p = Some(tp); }
        if let Some(reasoning) = options.reasoning { opts.reasoning = Some(reasoning); }
        if let Some(mr) = options.max_retries { opts.max_retries = Some(mr); }
        if let Some(timeout) = options.timeout { opts.timeout = Some(timeout); }
    }
}

fn apply_provider_response_mutation(
    msg: &mut llm_client::AssistantMessage,
    mutation: ProviderResponseMutation,
) {
    if let Some(content) = mutation.content {
        msg.content = content;
    }
    if let Some(stop_reason) = mutation.stop_reason {
        msg.stop_reason = stop_reason;
    }
}
```

- [ ] **Step 2: Rewrite AgentLoop struct and outer run method**

Replace the existing `AgentLoop` struct with:

```rust
pub struct AgentLoop {
    config: AgentLoopConfig,
}

#[derive(Debug, Clone)]
pub enum TurnResult {
    ToolUse,
    Stop,
    Error(AgentError),
}

impl AgentLoop {
    pub fn new(config: AgentLoopConfig) -> Self {
        Self { config }
    }

    pub async fn run(
        &self,
        initial_messages: Vec<AgentMessage>,
        signal: CancellationToken,
    ) -> Result<Vec<AgentMessage>, AgentError> {
        // on_before_agent_start hook
        let agent_start_ctx = BeforeAgentStartCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            system_prompt: self.config.system_prompt.clone(),
            messages: initial_messages.clone(),
            tools: build_tool_value_defs(&self.config.tools),
            model: self.config.model.clone(),
        };
        let agent_start_mutation = self.config.hook_dispatcher.on_before_agent_start(&agent_start_ctx).await;
        let system_prompt = agent_start_mutation.system_prompt.or_else(|| self.config.system_prompt.clone());
        let mut messages = agent_start_mutation.messages.unwrap_or(initial_messages);
        let mut new_messages: Vec<AgentMessage> = Vec::new();
        let mut turn_index: u64 = 0;
        let mut message_index: u64 = 0;

        (self.config.event_sink)(AgentEvent::AgentStart);

        loop {
            // Drain steer queue
            {
                let mut q = self.config.steer_queue.lock().expect("steer queue poisoned");
                messages.extend(q.drain(..));
            }

            // Inner turn loop
            loop {
                let result = self.run_turn(
                    &mut messages,
                    &mut new_messages,
                    &mut turn_index,
                    &mut message_index,
                    &system_prompt,
                    &signal,
                ).await;

                match result {
                    TurnResult::ToolUse => continue,
                    TurnResult::Stop => break,
                    TurnResult::Error(e) => {
                        (self.config.event_sink)(AgentEvent::Error { error: e.clone() });
                        (self.config.event_sink)(AgentEvent::AgentEnd { messages: messages.clone() });
                        self.config.hook_dispatcher.on_agent_end(&AgentEndCtx {
                            tenant_id: self.config.tenant_id.clone(),
                            session_id: self.config.session_id.clone(),
                            messages: messages.clone(),
                        }).await;
                        return Err(e);
                    }
                }
            }

            // Drain follow_up queue
            {
                let mut q = self.config.follow_up_queue.lock().expect("follow_up queue poisoned");
                let follow_ups: Vec<_> = q.drain(..).collect();
                if follow_ups.is_empty() { break; }
                messages.extend(follow_ups.clone());
                new_messages.extend(follow_ups);
            }
        }

        (self.config.event_sink)(AgentEvent::AgentEnd { messages: messages.clone() });
        self.config.hook_dispatcher.on_agent_end(&AgentEndCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            messages: messages.clone(),
        }).await;

        Ok(new_messages)
    }
}
```

- [ ] **Step 3: Implement run_turn inner method**

```rust
impl AgentLoop {
    async fn run_turn(
        &self,
        messages: &mut Vec<AgentMessage>,
        new_messages: &mut Vec<AgentMessage>,
        turn_index: &mut u64,
        message_index: &mut u64,
        system_prompt: &Option<String>,
        signal: &CancellationToken,
    ) -> TurnResult {
        *turn_index += 1;
        (self.config.event_sink)(AgentEvent::TurnStart { turn_index: *turn_index });

        // 1. on_context hook
        let after_context_messages = messages.clone();
        let ctx_ctx = ContextCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            messages: messages.clone(),
        };
        let mutation = self.config.hook_dispatcher.on_context(&ctx_ctx).await;
        let mut transformed = mutation.messages.unwrap_or_else(|| messages.clone());

        // 1.5 Resolve orphan tool calls
        resolve_orphan_tool_calls(&mut transformed);

        // 2. Build LlmContext
        let mut stream_opts = self.config.stream_options.clone();
        let mut ctx = LlmContext {
            system_prompt: system_prompt.clone(),
            messages: transformed,
            tools: build_tool_defs(&self.config.tools),
        };

        // 2.5 on_before_provider_request hook
        let provider_req_ctx = ProviderRequestCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            model: self.config.model.clone(),
            turn_index: *turn_index,
            system_prompt: ctx.system_prompt.clone(),
            messages: ctx.messages.clone(),
            tools: ctx.tools.clone(),
            options: ProviderStreamOptions::from_options(&self.config.stream_options),
        };
        let provider_req_mutation = self.config.hook_dispatcher.on_before_provider_request(&provider_req_ctx).await;
        apply_provider_request_mutation(&mut ctx, &mut stream_opts, provider_req_mutation);

        // 3. Call LLM with retry
        let (retry_count, mut assistant_msg) = match self.call_llm_with_retry(
            ctx, &stream_opts, *message_index, signal
        ).await {
            Ok(result) => result,
            Err(e) => {
                (self.config.event_sink)(AgentEvent::Error { error: e.clone() });
                return TurnResult::Error(e);
            }
        };

        // 3.5 on_after_provider_response hook
        let provider_resp_ctx = ProviderResponseCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            model: self.config.model.clone(),
            turn_index: *turn_index,
            attempt: retry_count,
            messages_before: after_context_messages,
            content: assistant_msg.content.clone(),
            stop_reason: assistant_msg.stop_reason.clone(),
        };
        let provider_resp_mutation = self.config.hook_dispatcher.on_after_provider_response(&provider_resp_ctx).await;
        apply_provider_response_mutation(&mut assistant_msg, provider_resp_mutation);

        // 4. Emit MessageEnd
        *message_index += 1;
        (self.config.event_sink)(AgentEvent::MessageEnd { message: AgentMessage::Assistant(assistant_msg.clone()) });
        new_messages.push(AgentMessage::Assistant(assistant_msg.clone()));
        messages.push(AgentMessage::Assistant(assistant_msg.clone()));

        // 5. Extract tool calls
        let tool_calls: Vec<&llm_client::ToolCall> = assistant_msg.content
            .iter()
            .filter_map(|c| match c {
                llm_client::Content::ToolCall(tc) => Some(tc),
                _ => None,
            })
            .collect();

        if tool_calls.is_empty() {
            // Check for error stop reasons
            match assistant_msg.stop_reason {
                llm_client::StopReason::Error | llm_client::StopReason::Aborted | llm_client::StopReason::Length => {
                    let err_msg = assistant_msg.error_message.clone()
                        .unwrap_or_else(|| format!("LLM returned stop reason: {:?}", assistant_msg.stop_reason));
                    return TurnResult::Error(AgentError::LlmResponseError(err_msg));
                }
                _ => {}
            }
            (self.config.event_sink)(AgentEvent::TurnEnd { turn_index: *turn_index, messages: messages.clone() });
            self.config.hook_dispatcher.on_turn_end(&TurnEndCtx {
                tenant_id: self.config.tenant_id.clone(),
                session_id: self.config.session_id.clone(),
                turn_index: *turn_index,
                messages: messages.clone(),
            }).await;
            return TurnResult::Stop;
        }

        // 6. Execute tools
        let tool_results = self.execute_tools(tool_calls, signal).await;
        let mut all_terminate = !tool_results.is_empty();
        for result in &tool_results {
            new_messages.push(AgentMessage::ToolResult(result.clone()));
            messages.push(AgentMessage::ToolResult(result.clone()));
            let terminated = result.details.as_ref()
                .and_then(|d| d.get("_terminate"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !terminated { all_terminate = false; }
        }

        (self.config.event_sink)(AgentEvent::TurnEnd { turn_index: *turn_index, messages: messages.clone() });
        self.config.hook_dispatcher.on_turn_end(&TurnEndCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            turn_index: *turn_index,
            messages: messages.clone(),
        }).await;

        if all_terminate {
            return TurnResult::Stop;
        }

        if assistant_msg.stop_reason == llm_client::StopReason::ToolUse {
            TurnResult::ToolUse
        } else {
            match assistant_msg.stop_reason {
                llm_client::StopReason::Error | llm_client::StopReason::Aborted | llm_client::StopReason::Length => {
                    let err_msg = assistant_msg.error_message.clone()
                        .unwrap_or_else(|| format!("LLM returned stop reason: {:?}", assistant_msg.stop_reason));
                    return TurnResult::Error(AgentError::LlmResponseError(err_msg));
                }
                _ => TurnResult::Stop,
            }
        }
    }
}
```

- [ ] **Step 4: Implement call_llm_with_retry**

```rust
impl AgentLoop {
    async fn call_llm_with_retry(
        &self,
        ctx: LlmContext,
        stream_opts: &StreamOptions,
        message_index: u64,
        signal: &CancellationToken,
    ) -> Result<(u32, llm_client::AssistantMessage), AgentError> {
        for attempt in 0..self.config.max_retries {
            if signal.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            match self.provider.stream(
                &self.config.model,
                ctx.clone(),
                stream_opts.clone(),
                signal.child_token(),
            ).await {
                Ok(mut stream) => {
                    (self.config.event_sink)(AgentEvent::MessageStart { message_index });
                    
                    let mut assistant_content: Vec<llm_client::Content> = Vec::new();
                    let mut text_accum: std::collections::BTreeMap<usize, String> = std::collections::BTreeMap::new();
                    let mut stop_reason = llm_client::StopReason::Stop;
                    let mut error_message: Option<String> = None;
                    let mut api = llm_client::Api {
                        provider: self.provider.provider_name().to_string(),
                        model: self.config.model.clone(),
                    };
                    let mut usage = llm_client::Usage {
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
                        total_tokens: 0,
                    };

                    while let Some(event) = stream.next().await {
                        if signal.is_cancelled() {
                            return Err(AgentError::Cancelled);
                        }
                        match event {
                            llm_client::AssistantMessageEvent::TextDelta { content_index, delta, .. } => {
                                text_accum.entry(content_index).or_default().push_str(&delta);
                                (self.config.event_sink)(AgentEvent::MessageUpdate { message_index, content_delta: delta });
                            }
                            llm_client::AssistantMessageEvent::TextEnd { content_index, text, .. } => {
                                let accumulated = text_accum.remove(&content_index).unwrap_or(text);
                                assistant_content.push(llm_client::Content::Text {
                                    text: accumulated,
                                    text_signature: None,
                                });
                            }
                            llm_client::AssistantMessageEvent::ToolCallEnd { tool_call, .. } => {
                                assistant_content.push(llm_client::Content::ToolCall(tool_call));
                            }
                            llm_client::AssistantMessageEvent::Done { reason, message } => {
                                assistant_content = message.content;
                                api = message.api;
                                usage = message.usage;
                                stop_reason = reason;
                                return Ok((attempt, llm_client::AssistantMessage {
                                    content: assistant_content,
                                    provider: api.provider.clone(),
                                    model: api.model.clone(),
                                    api,
                                    usage,
                                    stop_reason,
                                    response_id: None,
                                    error_message: None,
                                    timestamp: std::time::SystemTime::now(),
                                }));
                            }
                            llm_client::AssistantMessageEvent::Error { error } => {
                                error_message = error.error_message.clone();
                                stop_reason = error.stop_reason.clone();
                                if attempt < self.config.max_retries - 1 {
                                    match error.stop_reason {
                                        llm_client::StopReason::Error => {
                                            // Check if retryable
                                            let err_str = error.error_message.clone().unwrap_or_default().to_lowercase();
                                            if err_str.contains("rate limit") || err_str.contains("overloaded") {
                                                let delay = std::time::Duration::from_millis(100 * 2_u64.pow(attempt));
                                                tokio::time::sleep(delay).await;
                                                continue;
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                return Err(AgentError::LlmError(error));
                            }
                            _ => {}
                        }
                    }
                    
                    return Err(AgentError::LlmResponseError("stream ended without terminal event".to_string()));
                }
                Err(llm_client::LlmError::RateLimited) | Err(llm_client::LlmError::Overloaded) => {
                    if attempt < self.config.max_retries - 1 {
                        let delay = std::time::Duration::from_millis(100 * 2_u64.pow(attempt));
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(AgentError::LlmError(llm_client::LlmError::RateLimited));
                }
                Err(e) => return Err(AgentError::LlmError(e)),
            }
        }
        
        Err(AgentError::LlmResponseError("all retries exhausted".to_string()))
    }
}
```

- [ ] **Step 5: Implement execute_tools with sequential/parallel partitioning**

```rust
impl AgentLoop {
    async fn execute_tools(
        &self,
        tool_calls: Vec<&llm_client::ToolCall>,
        signal: &CancellationToken,
    ) -> Vec<llm_client::ToolResultMessage> {
        let mut results = Vec::new();

        let (sequential_calls, parallel_calls): (Vec<_>, Vec<_>) = tool_calls
            .into_iter()
            .partition(|tc| {
                self.config.tools.iter()
                    .find(|t| t.name() == tc.name)
                    .map(|t| t.execution_mode() == ToolExecutionMode::Sequential)
                    .unwrap_or(true)
            });

        for tc in sequential_calls {
            let result = self.execute_single_tool(tc, signal).await;
            results.push(result);
        }

        if !parallel_calls.is_empty() {
            let futures: Vec<_> = parallel_calls.iter()
                .map(|tc| self.execute_single_tool(tc, signal))
                .collect();
            let parallel_results = futures::future::join_all(futures).await;
            results.extend(parallel_results);
        }

        results
    }

    async fn execute_single_tool(
        &self,
        tc: &llm_client::ToolCall,
        signal: &CancellationToken,
    ) -> llm_client::ToolResultMessage {
        (self.config.event_sink)(AgentEvent::ToolExecutionStart {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
        });

        let tool = self.config.tools.iter().find(|t| t.name() == tc.name).cloned();
        let result = match tool {
            Some(tool) => {
                let executor = ToolExecutor::new(
                    self.config.tenant_id.clone(),
                    self.config.session_id.clone(),
                    self.config.hook_dispatcher.clone(),
                    tool,
                );
                let on_progress = |update: AgentToolProgressUpdate| {
                    (self.config.event_sink)(AgentEvent::ToolExecutionUpdate {
                        tool_call_id: tc.id.clone(),
                        content: update.content.clone(),
                    });
                };
                executor.execute_tool_call(tc, Some(&on_progress)).await
            }
            None => Err(AgentError::ToolNotFound(tc.name.clone())),
        };

        let result_msg = match result {
            Ok(msg) => msg,
            Err(e) => llm_client::ToolResultMessage {
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                content: vec![],
                details: Some(serde_json::json!({"error": e.to_string()})),
                is_error: true,
                timestamp: std::time::SystemTime::now(),
            },
        };

        (self.config.event_sink)(AgentEvent::ToolExecutionEnd {
            tool_call_id: tc.id.clone(),
            result: result_msg.clone(),
        });
        result_msg
    }
}
```

- [ ] **Step 6: Update existing tests to new signature**

Replace existing test struct `AgentLoop::new(...)` calls with `AgentLoopConfig`:

```rust
// In tests, replace:
// let loop_ = AgentLoop::new(tenant_id, session_id, model, provider, dispatcher, tools);
// With:
let config = AgentLoopConfig {
    tenant_id: "t1".to_string(),
    session_id: "s1".to_string(),
    model: "test".to_string(),
    provider,
    hook_dispatcher: dispatcher,
    tools: vec![],
    system_prompt: Some("You are helpful.".to_string()),
    stream_options: StreamOptions::default(),
    max_retries: 3,
    steer_queue: Arc::new(Mutex::new(vec![])),
    follow_up_queue: Arc::new(Mutex::new(vec![])),
    event_sink: Arc::new(|_| {}),
};
let loop_ = AgentLoop::new(config);
let results = loop_.run(initial_messages, CancellationToken::new()).await.unwrap();
```

- [ ] **Step 7: Verify all loop tests pass**

Run: `cargo test --package agent-core loop_`
Expected: PASS

---

## Phase 8: Error Type Update (P0)

### Task 8.1: Add CompactionFailed to AgentError (P0)

**Files:**
- Modify: `crates/agent-core/src/error.rs`

**Steps:**

- [ ] **Step 1: Add CompactionFailed variant**

```rust
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("tool not found: {0}")]
    ToolNotFound(String),

    #[error("tool execution failed: {0}")]
    ToolExecutionFailed(String),

    #[error("hook dispatch error: {0}")]
    HookDispatchError(String),

    #[error("llm error: {0}")]
    LlmError(#[from] llm_client::LlmError),

    #[error("llm response error: {0}")]
    LlmResponseError(String),

    #[error("cancelled")]
    Cancelled,

    #[error("compaction failed: {0}")]
    CompactionFailed(String),
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --package agent-core`
Expected: PASS

---

## Phase 9: SessionActor Refactor (P0)

### Task 9.1: Expand SessionActor to full spec (P0)

**Files:**
- Modify: `crates/agent-core/src/session.rs`

**Steps:**

- [ ] **Step 1: Update SessionActor fields**

Add:
- `entries: Arc<Mutex<Vec<SessionEntry>>>`
- `compaction_actor: Arc<CompactionActor>`
- `stream_options: StreamOptions`
- `max_retries: u32`
- `steer_queue: Arc<Mutex<Vec<AgentMessage>>>`
- `follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>`
- `recovery: RecoveryStateMachine`
- `event_listeners: Vec<Arc<dyn AgentEventListener>>`
- `event_tx: Option<mpsc::Sender<QueuedEvent>>`
- `event_processor_handle: Option<JoinHandle<()>>`
- `is_streaming: bool`

Remove:
- `messages: Vec<AgentMessage>` (replaced by entries)
- `steer_queue: Vec<AgentMessage>` (replaced by Arc<Mutex<...>>)
- `follow_up_queue: Vec<AgentMessage>` (replaced by Arc<Mutex<...>>)

- [ ] **Step 2: Implement event queue**

```rust
struct QueuedEvent {
    event: AgentEvent,
    new_messages: Vec<AgentMessage>,
}

impl SessionActor {
    fn spawn_event_processor(&mut self) -> mpsc::Sender<QueuedEvent> {
        let (tx, mut rx) = mpsc::channel::<QueuedEvent>(1024);
        let listeners = self.event_listeners.clone();
        let hook_dispatcher = self.hook_dispatcher.clone();
        let entries = self.entries.clone();
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();

        let handle = tokio::spawn(async move {
            while let Some(queued) = rx.recv().await {
                match &queued.event {
                    AgentEvent::TurnEnd { turn_index, messages } => {
                        let _ = hook_dispatcher.on_turn_end(&TurnEndCtx {
                            tenant_id: tenant_id.clone(),
                            session_id: session_id.clone(),
                            turn_index: *turn_index,
                            messages: messages.clone(),
                        }).await;
                    }
                    AgentEvent::AgentEnd { messages } => {
                        let _ = hook_dispatcher.on_agent_end(&AgentEndCtx {
                            tenant_id: tenant_id.clone(),
                            session_id: session_id.clone(),
                            messages: messages.clone(),
                        }).await;
                    }
                    _ => {}
                }

                for listener in &listeners {
                    let _ = listener.on_event(&queued.event).await;
                }

                {
                    let mut entries_guard = entries.lock().expect("entries poisoned");
                    for msg in &queued.new_messages {
                        entries_guard.push(SessionEntry::Message {
                            id: uuid::Uuid::new_v4(),
                            message: msg.clone(),
                        });
                    }
                }
            }
        });

        self.event_processor_handle = Some(handle);
        tx
    }
}
```

- [ ] **Step 3: Implement complete and continue_ methods**

```rust
impl SessionActor {
    pub async fn complete(&mut self, text: String) -> Result<String, AgentError> {
        let messages = self.prompt(text).await?;
        let text_content: Vec<String> = messages.iter().filter_map(|m| {
            if let AgentMessage::Assistant(a) = m {
                Some(a.content.iter().filter_map(|c| match c {
                    llm_client::Content::Text { text, .. } => Some(text.clone()),
                    _ => None,
                }).collect::<Vec<_>>().join(" "))
            } else {
                None
            }
        }).collect();
        Ok(text_content.join("\n"))
    }

    pub async fn continue_(&mut self) -> Result<Vec<AgentMessage>, AgentError> {
        self.run_with_messages(None).await
    }
}
```

- [ ] **Step 4: Rewrite SessionActor::new() and struct**

Replace existing struct with:

```rust
pub struct SessionActor {
    tenant_id: String,
    session_id: String,
    model: String,
    system_prompt: String,
    stream_options: llm_client::StreamOptions,
    max_retries: u32,
    provider: Arc<dyn llm_client::LlmProvider>,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    compaction_actor: Arc<CompactionActor>,
    tools: Vec<AgentToolRef>,
    entries: Arc<Mutex<Vec<SessionEntry>>>,
    store: Option<Arc<dyn SessionStore>>,
    steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    abort_token: CancellationToken,
    is_streaming: bool,
    recovery: RecoveryStateMachine,
    event_listeners: Vec<Arc<dyn AgentEventListener>>,
    event_tx: Option<mpsc::Sender<QueuedEvent>>,
    event_processor_handle: Option<tokio::task::JoinHandle<()>>,
}

struct QueuedEvent {
    event: AgentEvent,
    new_messages: Vec<AgentMessage>,
}

impl SessionActor {
    pub fn new(
        tenant_id: String,
        session_id: String,
        system_prompt: String,
        model: String,
        provider: Arc<dyn llm_client::LlmProvider>,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        compaction_actor: Arc<CompactionActor>,
        tools: Vec<AgentToolRef>,
        store: Option<Arc<dyn SessionStore>>,
    ) -> Self {
        let entries = Arc::new(Mutex::new(Vec::new()));
        let steer_queue = Arc::new(Mutex::new(Vec::new()));
        let follow_up_queue = Arc::new(Mutex::new(Vec::new()));
        
        let mut actor = Self {
            tenant_id: tenant_id.clone(),
            session_id: session_id.clone(),
            model: model.clone(),
            system_prompt: system_prompt.clone(),
            stream_options: llm_client::StreamOptions::default(),
            max_retries: 3,
            provider,
            hook_dispatcher: hook_dispatcher.clone(),
            compaction_actor,
            tools,
            entries,
            store,
            steer_queue,
            follow_up_queue,
            abort_token: CancellationToken::new(),
            is_streaming: false,
            recovery: RecoveryStateMachine::new(3),
            event_listeners: Vec::new(),
            event_tx: None,
            event_processor_handle: None,
        };

        let event_tx = actor.spawn_event_processor();
        actor.event_tx = Some(event_tx);

        // Fire on_session_start hook
        let tool_defs: Vec<serde_json::Value> = actor.tools.iter()
            .map(|t| serde_json::json!({
                "name": t.name(),
                "description": t.description(),
                "parameters": t.parameters(),
            }))
            .collect();
        let ctx = SessionCtx {
            tenant_id: tenant_id.clone(),
            session_id: session_id.clone(),
            system_prompt,
            tools: tool_defs,
        };
        let dispatcher = hook_dispatcher.clone();
        tokio::spawn(async move {
            dispatcher.on_session_start(&ctx).await;
        });

        actor
    }
}
```

- [ ] **Step 5: Implement event queue and run_with_messages**

```rust
impl SessionActor {
    fn spawn_event_processor(&mut self) -> mpsc::Sender<QueuedEvent> {
        let (tx, mut rx) = mpsc::channel::<QueuedEvent>(1024);
        let listeners = self.event_listeners.clone();
        let hook_dispatcher = self.hook_dispatcher.clone();
        let entries = self.entries.clone();
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();

        let handle = tokio::spawn(async move {
            while let Some(queued) = rx.recv().await {
                match &queued.event {
                    AgentEvent::TurnEnd { turn_index, messages } => {
                        let _ = hook_dispatcher.on_turn_end(&TurnEndCtx {
                            tenant_id: tenant_id.clone(),
                            session_id: session_id.clone(),
                            turn_index: *turn_index,
                            messages: messages.clone(),
                        }).await;
                    }
                    AgentEvent::AgentEnd { messages } => {
                        let _ = hook_dispatcher.on_agent_end(&AgentEndCtx {
                            tenant_id: tenant_id.clone(),
                            session_id: session_id.clone(),
                            messages: messages.clone(),
                        }).await;
                    }
                    _ => {}
                }

                for listener in &listeners {
                    let _ = listener.on_event(&queued.event).await;
                }

                {
                    let mut entries_guard = entries.lock().expect("entries poisoned");
                    for msg in &queued.new_messages {
                        entries_guard.push(SessionEntry::Message {
                            id: uuid::Uuid::new_v4(),
                            message: msg.clone(),
                        });
                    }
                }
            }
        });

        self.event_processor_handle = Some(handle);
        tx
    }

    pub async fn prompt(&mut self, text: String) -> Result<Vec<AgentMessage>, AgentError> {
        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text { text, text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        });
        {
            let mut entries = self.entries.lock().expect("entries poisoned");
            entries.push(SessionEntry::Message {
                id: uuid::Uuid::new_v4(),
                message: user_msg.clone(),
            });
        }
        self.run_with_messages(None).await
    }

    pub async fn continue_(&mut self) -> Result<Vec<AgentMessage>, AgentError> {
        self.run_with_messages(None).await
    }

    async fn run_with_messages(&mut self, _add_user_msg: Option<String>) -> Result<Vec<AgentMessage>, AgentError> {
        self.is_streaming = true;
        self.abort_token = CancellationToken::new();

        let messages = {
            let entries = self.entries.lock().expect("entries poisoned");
            SessionContextBuilder::build_context(&*entries)
        };

        let event_tx = self.event_tx.clone();
        let event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync> = Arc::new(move |event| {
            if let Some(tx) = &event_tx {
                let new_messages = match &event {
                    AgentEvent::MessageEnd { message } => vec![message.clone()],
                    _ => vec![],
                };
                if tx.try_send(QueuedEvent { event, new_messages }).is_err() {
                    tracing::warn!("event queue full, dropping event");
                }
            }
        });

        let config = AgentLoopConfig {
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            hook_dispatcher: self.hook_dispatcher.clone(),
            tools: self.tools.clone(),
            system_prompt: Some(self.system_prompt.clone()),
            stream_options: self.stream_options.clone(),
            max_retries: self.max_retries,
            event_sink,
            steer_queue: self.steer_queue.clone(),
            follow_up_queue: self.follow_up_queue.clone(),
        };

        let new_msgs = match AgentLoop::new(config).run(messages, self.abort_token.child_token()).await {
            Ok(msgs) => {
                self.is_streaming = false;
                
                // Post-processing: recovery and compaction
                if let Some(AgentMessage::Assistant(assistant)) = msgs.iter().rfind(|m| matches!(m, AgentMessage::Assistant(_))) {
                    let action = self.recovery.evaluate(assistant);
                    match action {
                        RecoveryAction::RetryAfterCompaction { reason } => {
                            if let Err(e) = self.run_auto_compaction(reason, true).await {
                                return Err(e);
                            }
                            self.recovery.mark_success();
                            return self.continue_().await;
                        }
                        RecoveryAction::RetryAfterBackoff { delay_ms } => {
                            if let Some(tx) = &self.event_tx {
                                let _ = tx.send(QueuedEvent {
                                    event: AgentEvent::AutoRetryStart {
                                        attempt: self.recovery.retry_count,
                                        max_attempts: self.recovery.max_retries,
                                        delay_ms,
                                    },
                                    new_messages: vec![],
                                });
                            }
                            tokio::select! {
                                _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                                _ = self.abort_token.cancelled() => {
                                    if let Some(tx) = &self.event_tx {
                                        let _ = tx.send(QueuedEvent {
                                            event: AgentEvent::AutoRetryEnd { success: false, error: Some("cancelled".to_string()) },
                                            new_messages: vec![],
                                        });
                                    }
                                    self.recovery.reset();
                                    return Ok(vec![]);
                                }
                            }
                            if let Some(tx) = &self.event_tx {
                                let _ = tx.send(QueuedEvent {
                                    event: AgentEvent::AutoRetryEnd { success: true, error: None },
                                    new_messages: vec![],
                                });
                            }
                            self.recovery.mark_success();
                            return self.continue_().await;
                        }
                        RecoveryAction::Abort { reason } => {
                            if let Some(tx) = &self.event_tx {
                                let _ = tx.send(QueuedEvent {
                                    event: AgentEvent::AutoRetryEnd { success: false, error: Some(reason.clone()) },
                                    new_messages: vec![],
                                });
                            }
                            self.recovery.mark_success();
                            return Ok(vec![]);
                        }
                        RecoveryAction::Continue => {
                            self.recovery.mark_success();
                            if let Err(e) = self.check_threshold_compaction(assistant).await {
                                tracing::warn!("threshold compaction check failed: {}", e);
                            }
                        }
                    }
                }
                Ok(msgs)
            }
            Err(e) => {
                self.is_streaming = false;
                Err(e)
            }
        };

        new_msgs
    }
}
```

- [ ] **Step 6: Implement compaction and public API methods**

```rust
impl SessionActor {
    async fn run_auto_compaction(&mut self, reason: CompactReason, will_retry: bool) -> Result<(), AgentError> {
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(QueuedEvent {
                event: AgentEvent::CompactionStart { reason: reason.clone() },
                new_messages: vec![],
            });
        }

        let entries_guard = self.entries.lock().expect("entries poisoned");
        let preparation = self.compaction_actor.prepare(&*entries_guard)
            .map_err(|e| AgentError::CompactionFailed(e.to_string()))?;

        let compact_ctx = CompactCtx {
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            preparation,
            entries: (*entries_guard).clone(),
            reason: reason.clone(),
        };
        drop(entries_guard);

        let decision = self.hook_dispatcher.on_before_compact(&compact_ctx).await;

        let result = match decision {
            CompactDecision::Block { reason } => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(QueuedEvent {
                        event: AgentEvent::CompactionEnd {
                            reason: reason.clone(),
                            result: None,
                            aborted: true,
                            will_retry: false,
                            error_message: Some(reason.clone()),
                        },
                        new_messages: vec![],
                    });
                }
                return Ok(());
            }
            CompactDecision::Replace { result } => result,
            CompactDecision::Continue => {
                let entries_guard = self.entries.lock().expect("entries poisoned");
                self.compaction_actor.compact(&*entries_guard, &self.abort_token.child_token()).await
                    .map_err(|e| AgentError::CompactionFailed(e.to_string()))?
            }
        };

        let compaction_entry = SessionEntry::Compaction {
            id: uuid::Uuid::new_v4(),
            summary: result.summary.clone(),
            first_kept_entry_id: result.first_kept_entry_id,
            tokens_before: result.tokens_before,
            details: result.details.clone(),
            from_extension: matches!(decision, CompactDecision::Replace { .. }),
            timestamp: std::time::SystemTime::now(),
        };

        {
            let mut entries = self.entries.lock().expect("entries poisoned");
            entries.push(compaction_entry);
        }

        if let Some(tx) = &self.event_tx {
            let _ = tx.send(QueuedEvent {
                event: AgentEvent::CompactionEnd {
                    reason: reason.clone(),
                    result: Some(result.clone()),
                    aborted: false,
                    will_retry,
                    error_message: None,
                },
                new_messages: vec![],
            });
        }

        if will_retry {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        Ok(())
    }

    async fn check_threshold_compaction(&mut self, last_assistant: &llm_client::AssistantMessage) -> Result<(), AgentError> {
        if !self.compaction_actor.config.enabled {
            return Ok(());
        }

        if last_assistant.stop_reason == llm_client::StopReason::Aborted {
            return Ok(());
        }

        let context_tokens = {
            let entries = self.entries.lock().expect("entries poisoned");
            crate::compaction::estimate_context_tokens(&*entries)
        };

        let context_window = self.model_context_window();
        let config = &self.compaction_actor.config;

        if context_window > 0 && context_tokens > context_window.saturating_sub(config.reserve_tokens) {
            self.run_auto_compaction(CompactReason::Threshold, false).await?;
        }

        Ok(())
    }

    fn model_context_window(&self) -> usize {
        match self.model.as_str() {
            "gpt-4" | "gpt-4o" => 128_000,
            "gpt-4-turbo" => 128_000,
            "gpt-3.5-turbo" => 16_385,
            "claude-3-5-sonnet" | "claude-3-5-sonnet-20241022" => 200_000,
            "claude-3-opus" | "claude-3-opus-20240229" => 200_000,
            "gemini-1.5-pro" => 2_000_000,
            "gemini-1.5-flash" => 1_000_000,
            _ => 0,
        }
    }

    pub async fn restore(&mut self) -> Result<usize, AgentError> {
        if let Some(ref store) = self.store {
            let loaded = store.load_session(&self.tenant_id, &self.session_id).await?;
            let count = loaded.len();
            if count > 0 {
                let mut entries = self.entries.lock().expect("entries poisoned");
                for msg in loaded {
                    entries.push(SessionEntry::Message {
                        id: uuid::Uuid::new_v4(),
                        message: msg,
                    });
                }
            }
            Ok(count)
        } else {
            Ok(0)
        }
    }

    pub async fn flush(&self) -> Result<(), AgentError> {
        if let Some(ref store) = self.store {
            let entries = self.entries.lock().expect("entries poisoned");
            let messages: Vec<AgentMessage> = entries.iter().filter_map(|e| match e {
                SessionEntry::Message { message, .. } => Some(message.clone()),
                _ => None,
            }).collect();
            store.save_session(&self.tenant_id, &self.session_id, &messages).await?;
        }
        Ok(())
    }

    pub fn entries(&self) -> Vec<SessionEntry> {
        self.entries.lock().expect("entries poisoned").clone()
    }

    pub fn messages(&self) -> Vec<AgentMessage> {
        let entries = self.entries.lock().expect("entries poisoned");
        SessionContextBuilder::build_context(&*entries)
    }

    pub fn steer(&self, message: AgentMessage) {
        let mut q = self.steer_queue.lock().expect("steer queue poisoned");
        q.push(message);
    }

    pub fn follow_up(&self, message: AgentMessage) {
        let mut q = self.follow_up_queue.lock().expect("follow_up queue poisoned");
        q.push(message);
    }

    pub fn abort(&self) {
        self.abort_token.cancel();
    }

    pub fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    pub fn add_event_listener(&mut self, listener: Arc<dyn AgentEventListener>) {
        self.event_listeners.push(listener);
    }

    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = prompt;
    }

    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    pub fn set_tools(&mut self, tools: Vec<AgentToolRef>) {
        self.tools = tools;
    }

    pub fn set_stream_options(&mut self, options: llm_client::StreamOptions) {
        self.stream_options = options;
    }

    pub fn set_max_retries(&mut self, max_retries: u32) {
        self.max_retries = max_retries;
    }
}
```

- [ ] **Step 7: Update tests**

Replace existing session tests to use new `SessionActor::new()` signature (adds `compaction_actor` parameter):

```rust
// In tests:
let compaction_actor = Arc::new(CompactionActor::new(
    CompactionConfig::default(),
    provider.clone(),
    "test".to_string(),
    Arc::new(DefaultFileOperationExtractor::default()),
));
let mut session = SessionActor::new(
    "t1".to_string(),
    "s1".to_string(),
    "You are helpful.".to_string(),
    "echo".to_string(),
    provider,
    dispatcher,
    compaction_actor,
    vec![],
    None,
);
```

- [ ] **Step 8: Verify session tests pass**

Run: `cargo test --package agent-core session`
Expected: PASS

---

## Phase 10: lib.rs and Integration (P1)

### Task 10.1: Update lib.rs exports (P1)

**Files:**
- Modify: `crates/agent-core/src/lib.rs`

**Steps:**

- [ ] **Step 1: Add all new module exports**

Update `src/lib.rs`:

```rust
pub mod compaction;
pub mod context;
pub mod error;
pub mod error_recovery;
pub mod events;
pub mod file_ops;
pub mod hook_dispatcher;
pub mod loop_;
pub mod mutations;
pub mod provider_opts;
pub mod session;
pub mod session_entry;
pub mod store;
pub mod tool;
pub mod types;

pub use compaction::{CompactionActor, CompactionConfig, CompactionResult, CompactionPreparation, CompactionError};
pub use context::*;
pub use error::AgentError;
pub use error_recovery::{RecoveryStateMachine, RecoveryAction};
pub use events::{AgentEvent, AgentEventListener};
pub use file_ops::{FileOperationExtractor, FileOperations, DefaultFileOperationExtractor};
pub use hook_dispatcher::HookDispatcher;
pub use loop_::{AgentLoop, AgentLoopConfig, resolve_orphan_tool_calls};
pub use mutations::*;
pub use provider_opts::ProviderStreamOptions;
pub use session::SessionActor;
pub use session_entry::{SessionEntry, CompactionDetails, SessionContextBuilder};
pub use store::SessionStore;
pub use tool::ToolExecutor;
pub use types::*;
```

- [ ] **Step 2: Final verification**

Run: `cargo test --package agent-core`
Expected: ALL tests PASS

Run: `cargo check --package agent-core`
Expected: PASS (no warnings)

---

## Phase 11: Documentation (P1)

### Task 11.1: Update README.md (P1)

**Files:**
- Modify: `crates/agent-core/README.md`

**Steps:**

- [ ] **Step 1: Update module table**

Add new modules to the table:
- `events` - AgentEvent enum and AgentEventListener trait
- `session_entry` - SessionEntry, CompactionDetails, SessionContextBuilder
- `compaction` - CompactionActor
- `file_ops` - FileOperationExtractor
- `error_recovery` - RecoveryStateMachine
- `provider_opts` - ProviderStreamOptions

- [ ] **Step 2: Add new design sections**

Add sections for:
- Event system (AgentEvent, event queue, listeners)
- Compaction (cut-point algorithm, LLM summary, mid-turn split)
- Error recovery (RecoveryStateMachine, retry logic)
- SessionEntry model (messages vs entries, context building)

---

## Summary

This plan evolves agent-core from a basic skeleton to a production-ready agent loop runtime with:

1. **Event system** - Observable agent lifecycle via AgentEvent
2. **SessionEntry model** - Compaction boundaries and context building
3. **Expanded hooks** - 5 new HookDispatcher methods for fine-grained extension control
4. **Error recovery** - Automatic retry and compaction on context overflow
5. **Orphan resolution** - Safe handling of incomplete tool call histories
6. **LLM retry** - Exponential backoff for rate-limited providers
7. **Compaction** - Token-aware context window management with LLM summarization

## Phase 优先级总览

| Phase | 内容 | 优先级 | 阻塞关系 | 预估时间 |
|---|---|---|---|---|
| Phase 0 | Foundation Types (8 Ctx + 5 Mutation) | **P0** | 阻塞 extensions 全部 | ~1h |
| Phase 1 | Events System | **P0** | - | ~30 min |
| Phase 2 | SessionEntry and Context Builder | **P0** | - | ~1h |
| Phase 3 | Error Recovery | **P0** | - | ~1h |
| Phase 4 | File Operations Extractor | **P0** | - | ~30 min |
| Phase 5 | CompactionActor | **P0** | - | ~3h |
| Phase 6 | Tool Executor Update | **P0** | - | ~1h |
| Phase 7 | AgentLoop Refactor | **P0** | 风险最高 | ~4h |
| Phase 8 | Error Type Update | **P0** | - | ~15 min |
| Phase 9 | SessionActor Refactor | **P0** | - | ~3h |
| Phase 10 | lib.rs and Integration | **P1** | - | ~30 min |
| Phase 11 | Documentation | **P1** | - | ~30 min |

**P0 总计**: ~16h | **P1 总计**: ~1h | **全部总计**: ~17h

**Dependencies:** ai-provider (already implemented), uuid (new)
**Risk areas:**
- AgentLoop refactor (Task 7.2) is large and affects all existing tests — **建议拆分为子任务**
- SessionActor refactor (Task 9.1) changes core data model from Vec<AgentMessage> to Vec<SessionEntry>
- CompactionActor LLM integration requires a real LlmProvider in production (tests can use mock)

**联合开发顺序:**
```
第 1 步: agent-core Phase 0 (P0) — 阻塞级，单独完成
         └─ 完成后通知 extensions 启动
第 2 步: agent-core Phase 1-4 (P0) + ai-provider v0.2 P1 (P1) 并行
第 3 步: agent-core Phase 5-7 (P0) + extensions Phase 1-5 (P0) 并行
第 4 步: agent-core Phase 8-9 (P0) + extensions Phase 6-7 (P1) 并行
第 5 步: agent-core Phase 10-11 (P1) + ai-provider v0.2 P3 (P2) 可选
```

**Next steps after this plan:**
1. Implement tenant crate (TenantManager, session registry)
2. Implement api-gateway crate (axum + SSE)
