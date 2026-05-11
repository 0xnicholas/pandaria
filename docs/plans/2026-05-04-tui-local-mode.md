# Pandaria TUI — Local Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate `agent-core` and `llm-client` into the TUI crate so the TUI works as a self-contained standalone CLI tool without requiring a running pandaria server. Replace HTTP/SSE backend with an in-process agent loop, preserving the existing `ServerEvent`-based UI rendering path.

**Architecture:** Bottom-up: event mapping → StreamingProvider → Backend trait → LocalBackend → Config → App → Main. Each layer is independently testable.

**Tech Stack:** Rust 2024 edition, tokio, ratatui 0.29, crossterm 0.28, clap 4, async-trait, secrecy, serde_json. Add deps on agent-core + llm-client (workspace crates).

**Spec Reference:** `docs/specs/2026-05-04-tui-local-mode.md`

---

## Current State

The TUI crate (`crates/tui/`) is a fully-featured terminal chat client that communicates with a pandaria server via HTTP REST + SSE. All slash commands (`/new`, `/switch`, `/model`, `/clear`, `/quit`) and message submission go through `RestClient` → HTTP calls to a server that doesn't exist yet.

The `agent-core`, `llm-client`, and `extensions` crates have all the agent runtime logic but aren't wired to any binary. The root `src/main.rs` is an empty stub.

**Working today:**
- Terminal UI rendering (ratatui widgets, overlays, markdown)
- `ServerEvent` deserialization and state mapping
- `AppState` machine (Disconnected/Connected/Busy)
- `Command` parser (`/quit`, `/new`, `/switch`, `/model`, `/clear`, `/help`, `/connect`, `/auth`, `/tokens`)
- `InputBar` (single-line editor), `SpinnerWidget`, `ChatView`, overlay system

**Not working:**
- Any server-dependent operation (`/new` calls `rest.create_session()` → dead HTTP call)
- Message submission (`submit_input()` calls `rest.send_message()` → dead HTTP call)
- SSE streaming (no server to connect to)

---

## File Map

### New Files
| File | Phase | Purpose |
|---|---|---|
| `crates/tui/src/backend.rs` | 4 | `Backend` trait + `mod` declarations |
| `crates/tui/src/backend/event_mapper.rs` | 2 | `map_stream_event()`, `EventMapperState` |
| `crates/tui/src/backend/streaming_provider.rs` | 3 | `StreamingProvider` wrapping `LlmProvider` |
| `crates/tui/src/backend/local.rs` | 4 | `LocalBackend`: session CRUD, `send_message`, interrupt |

### Modified Files
| File | Phase | Change |
|---|---|---|
| `crates/tui/Cargo.toml` | 1 | Add `agent-core`, `llm-client`, `secrecy` deps |
| `crates/tui/src/lib.rs` | 1 | Add `pub mod backend;` |
| `crates/tui/src/config.rs` | 5 | Add `LlmConfig`, make `ServerConfig` optional, new CLI args |
| `crates/tui/src/app.rs` | 6 | Replace `RestClient` + `reqwest_client` with `Box<dyn Backend>`. Update `App::new()`, `submit_input()`, `handle_overlay_confirm()` |
| `crates/tui/src/main.rs` | 7 | Provider construction, mode selection, `LocalBackend` init |

### Unchanged Files
All UI widgets (`chat_view.rs`, `tool_call.rs`, `thinking.rs`, `status_bar.rs`, `spinner.rs`, `header.rs`, `session_tabs.rs`), overlays (`command_palette.rs`, `help.rs`, `model_selector.rs`, `session_list.rs`), `state.rs`, `command.rs`, `markdown.rs`, `paste.rs`, `client/*` (preserved for future `HttpBackend`), `ui/theme.rs`.

---

## Phase 1: Crate Setup

#### Task 1.1: Add workspace dependencies and module declaration

**Files:**
- Modify: `crates/tui/Cargo.toml`
- Modify: `crates/tui/src/lib.rs`

**Steps:**

- [ ] **Step 1: Add deps to Cargo.toml**

```toml
[dependencies]
# ... existing deps ...

# Internal workspace crates
agent-core = { path = "../agent-core" }
llm-client = { path = "../llm-client" }

# Secret handling for API keys
secrecy = "0.8"

# Session ID generation
uuid = { version = "1", features = ["v4"] }
```

- [ ] **Step 2: Add `pub mod backend` to lib.rs**

```rust
// In crates/tui/src/lib.rs, add:
pub mod backend;
```

- [ ] **Step 3: Verify**

```bash
# Check that crate compiles with new deps
cargo check -p tui
```

Expected: No errors. Dependencies resolve. Module `backend` is not found yet (expected — will be created in subsequent phases).

---

## Phase 2: Event Mapper

#### Task 2.1: Implement `EventMapperState` and `map_stream_event()`

**Files:**
- Create: `crates/tui/src/backend/event_mapper.rs`
- Create: `crates/tui/src/backend/mod.rs` (with `pub mod event_mapper;`)

**Steps:**

- [ ] **Step 1: Define `EventMapperState` struct**

```rust
use std::collections::{BTreeMap, HashMap};

/// Mutable state for the event mapper, held by StreamingProvider.
pub struct EventMapperState {
    /// Buffered tool call deltas, keyed by content_index.
    /// BTreeMap guarantees insertion order for deterministic replay.
    pub pending_tool_deltas: BTreeMap<usize, Vec<String>>,

    /// Per-call_id → tool_name mapping, populated when ToolCallEnd
    /// reveals the full ToolCall. Read by the spawned task to provide
    /// names for ToolCallDone events.
    pub tool_names: HashMap<String, String>,

    /// Current turn index (increments per Done event).
    pub turn_index: u64,
}

impl EventMapperState {
    pub fn new() -> Self {
        Self {
            pending_tool_deltas: BTreeMap::new(),
            tool_names: HashMap::new(),
            turn_index: 0,
        }
    }
}
```

- [ ] **Step 2: Implement `map_stream_event()`**

```rust
use crate::client::model::{ServerEvent, UsageInfo};
use llm_client::streaming::AssistantMessageEvent;

pub fn map_stream_event(
    event: &AssistantMessageEvent,
    state: &mut EventMapperState,
) -> Vec<ServerEvent> {
    match event {
        AssistantMessageEvent::Start { .. } => {
            vec![ServerEvent::MessageStart {
                message_index: state.turn_index,
            }]
        }
        AssistantMessageEvent::TextDelta { delta, .. } => {
            vec![ServerEvent::TextDelta {
                delta: delta.clone(),
            }]
        }
        AssistantMessageEvent::ThinkingDelta {
            content_index, delta, ..
        } => {
            vec![ServerEvent::ThinkingDelta {
                content_index: *content_index,
                delta: delta.clone(),
            }]
        }
        // ToolCallStart only has content_index — no call_id or name yet.
        // Buffer deltas by content_index until ToolCallEnd arrives.
        AssistantMessageEvent::ToolCallStart {
            content_index, ..
        } => {
            state
                .pending_tool_deltas
                .entry(*content_index)
                .or_default();
            Vec::new()
        }
        AssistantMessageEvent::ToolCallDelta {
            content_index, delta, ..
        } => {
            state
                .pending_tool_deltas
                .entry(*content_index)
                .or_default()
                .push(delta.clone());
            Vec::new()
        }
        // ToolCallEnd reveals the full ToolCall with id and name.
        // Emit ToolCallStarted + replay any buffered deltas.
        AssistantMessageEvent::ToolCallEnd {
            content_index,
            tool_call,
            ..
        } => {
            state
                .tool_names
                .insert(tool_call.id.clone(), tool_call.name.clone());

            let mut events = Vec::new();
            events.push(ServerEvent::ToolCallStarted {
                call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
            });

            if let Some(deltas) =
                state.pending_tool_deltas.remove(content_index)
            {
                for delta in deltas {
                    events.push(ServerEvent::ToolCallDelta {
                        call_id: tool_call.id.clone(),
                        delta,
                    });
                }
            }
            events
        }
        // Done fires for every turn. Suppressed — the spawned task emits
        // a single TurnEnd after AgentLoop::run() completes.
        AssistantMessageEvent::Done { .. } => {
            state.turn_index += 1;
            Vec::new()
        }
        AssistantMessageEvent::Error { error } => {
            vec![ServerEvent::Error {
                code: format!("{:?}", error.stop_reason).to_lowercase(),
                message: error
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "Unknown error".to_string()),
            }]
        }
        _ => Vec::new(),
    }
}
```

- [ ] **Step 3: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a minimal AssistantMessage for event construction.
    fn make_assistant() -> llm_client::AssistantMessage {
        llm_client::AssistantMessage {
            content: vec![],
            provider: "test".into(),
            model: "test".into(),
            api: llm_client::Api {
                provider: "test".into(),
                model: "test".into(),
            },
            usage: llm_client::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 0,
            },
            stop_reason: llm_client::StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        }
    }

    #[test]
    fn test_text_delta_mapping() {
        let mut state = EventMapperState::new();
        let event = AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "Hello".to_string(),
            partial: make_assistant(),
        };
        let results = map_stream_event(&event, &mut state);
        assert_eq!(results.len(), 1);
        assert!(matches!(&results[0], ServerEvent::TextDelta { delta } if delta == "Hello"));
    }

    #[test]
    fn test_thinking_delta_mapping() {
        let mut state = EventMapperState::new();
        let event = AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "Hmm...".to_string(),
            partial: make_assistant(),
        };
        let results = map_stream_event(&event, &mut state);
        assert_eq!(results.len(), 1);
        assert!(matches!(&results[0], ServerEvent::ThinkingDelta { content_index: 0, delta } if delta == "Hmm..."));
    }

    #[test]
    fn test_tool_call_end_emits_started_and_buffered_deltas() {
        let mut state = EventMapperState::new();

        // Simulate ToolCallStart (just registers content_index)
        let start = AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            partial: make_assistant(),
        };
        let r = map_stream_event(&start, &mut state);
        assert!(r.is_empty());

        // Simulate ToolCallDelta (buffered)
        let delta = AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "{\"path\":".to_string(),
            partial: make_assistant(),
        };
        let r = map_stream_event(&delta, &mut state);
        assert!(r.is_empty());

        // Simulate ToolCallEnd (emits Started + buffered deltas)
        let end = AssistantMessageEvent::ToolCallEnd {
            content_index: 0,
            tool_call: llm_client::ToolCall {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
                thought_signature: None,
            },
            partial: make_assistant(),
        };
        let results = map_stream_event(&end, &mut state);
        assert_eq!(results.len(), 2);
        assert!(matches!(&results[0], ServerEvent::ToolCallStarted { call_id, name } if call_id == "call_1" && name == "read_file"));
        assert!(matches!(&results[1], ServerEvent::ToolCallDelta { call_id, delta } if call_id == "call_1" && delta == "{\"path\":"));
    }

    #[test]
    fn test_done_is_suppressed() {
        let mut state = EventMapperState::new();
        let done = AssistantMessageEvent::Done {
            reason: llm_client::StopReason::Stop,
            message: make_assistant(),
        };
        let results = map_stream_event(&done, &mut state);
        assert!(results.is_empty());
        assert_eq!(state.turn_index, 1); // turn index still increments
    }

    #[test]
    fn test_error_mapping() {
        let mut state = EventMapperState::new();
        let mut partial = make_assistant();
        partial.error_message = Some("bad request".to_string());
        let error_event = AssistantMessageEvent::Error {
            error: partial,
        };
        let results = map_stream_event(&error_event, &mut state);
        assert_eq!(results.len(), 1);
        assert!(matches!(&results[0], ServerEvent::Error { code, message } if message == "bad request"));
    }
}
```

**Note:** `Usage` does not derive `Default` — construct explicitly as shown.

- [ ] **Step 4: Write backend/mod.rs**

```rust
pub mod event_mapper;
```

- [ ] **Step 5: Verify and commit**

```bash
cargo test -p tui event_mapper
```

Expected: All 5 tests pass.

```bash
git add crates/tui/src/backend/mod.rs crates/tui/src/backend/event_mapper.rs
git commit -m "feat(tui): add event_mapper — AssistantMessageEvent → ServerEvent mapping"
```

---

## Phase 3: StreamingProvider

#### Task 3.1: Implement `StreamingProvider` wrapping `LlmProvider`

**Files:**
- Create: `crates/tui/src/backend/streaming_provider.rs`
- Modify: `crates/tui/src/backend/mod.rs`

**Steps:**

- [ ] **Step 1: Define `StreamingProvider` struct**

```rust
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_client::{
    AssistantMessageEventStream, LlmContext, LlmError, LlmProvider, StreamOptions,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::client::model::ServerEvent;
use crate::backend::event_mapper::{EventMapperState, map_stream_event};

pub struct StreamingProvider {
    inner: Arc<dyn LlmProvider>,
    tx: mpsc::Sender<ServerEvent>,
    state: Mutex<EventMapperState>,
}

impl StreamingProvider {
    pub fn new(
        inner: Arc<dyn LlmProvider>,
        tx: mpsc::Sender<ServerEvent>,
    ) -> Self {
        Self {
            inner,
            tx,
            state: Mutex::new(EventMapperState::new()),
        }
    }

    /// Read the accumulated tool_names map (for ToolCallDone emission
    /// by the spawned task after AgentLoop completes).
    pub fn tool_names(&self) -> HashMap<String, String> {
        self.state.lock().expect("mutex poisoned").tool_names.clone()
    }
}
```

**Import note:** `std::sync::Mutex` is used here (not `tokio::sync::Mutex`) because the lock is never held across `.await` — `map_stream_event` is a synchronous function called inside the stream wrapper's `poll_next`.

- [ ] **Step 2: Implement `LlmProvider` for `StreamingProvider`**

The key challenge: the forwarded stream must be consumed by the agent loop (via `AssistantMessageEventStream`), while simultaneously mapping events to TUI `ServerEvent` messages. The cleanest approach uses `AssistantMessageEventStream::new()` with a spawned forwarding task.

```rust
#[async_trait]
impl LlmProvider for StreamingProvider {
    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    fn models(&self) -> Vec<String> {
        self.inner.models()
    }

    async fn stream(
        &self,
        model: &str,
        context: LlmContext,
        options: StreamOptions,
        signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError> {
        use futures::StreamExt;

        let mut inner_stream = self.inner.stream(
            model, context, options, signal,
        ).await?;

        let (mut event_stream, event_tx) =
            AssistantMessageEventStream::new(16);

        let tx = self.tx.clone();
        let state = self.state.clone();

        tokio::spawn(async move {
            while let Some(event) = inner_stream.next().await {
                {
                    let mut s = state.lock().expect("mutex poisoned");
                    let tui_events = map_stream_event(&event, &mut s);
                    for tui_event in tui_events {
                        let _ = tx.try_send(tui_event);
                    }
                }
                if event_tx.send(event).await.is_err() {
                    break;
                }
            }
        });

        Ok(event_stream)
    }
}
```

**Design notes:**
- `AssistantMessageEventStream::new(capacity)` returns a `(Self, mpsc::Sender<AssistantMessageEvent>)` pair — stream on one end, sender on the other.
- The spawned task consumes the inner stream, maps events (writing to `state` via `Mutex`), forwards TUI events via `tx.try_send()`, and forwards original events to `event_tx` for the agent loop.
- `std::sync::Mutex` is used because the lock scope is limited to `map_stream_event()` which is synchronous — never held across `.await`.
- If the `tx` channel is full, the event is silently dropped (backpressure — see spec §5.3).

- [ ] **Step 4: Write backend/mod.rs update**

Add to `backend/mod.rs`:
```rust
pub mod streaming_provider;
```

- [ ] **Step 5: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::streaming::AssistantMessageEvent;
    use tokio::sync::mpsc;

    /// Mock provider that sends a sequence of events then completes.
    struct MockProvider {
        events: Vec<AssistantMessageEvent>,
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn provider_name(&self) -> &str { "mock" }
        fn models(&self) -> Vec<String> { vec!["mock".into()] }
        async fn stream(
            &self,
            _model: &str,
            _context: LlmContext,
            _options: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<AssistantMessageEventStream, LlmError> {
            let (mut stream, tx) = AssistantMessageEventStream::new(8);
            let events = self.events.clone();
            tokio::spawn(async move {
                for event in events {
                    if tx.send(event).await.is_err() { break; }
                }
            });
            Ok(stream)
        }
    }

    #[tokio::test]
    async fn test_events_forwarded_to_tui_channel() {
        let events = vec![
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "Hello".into(),
                partial: make_minimal_assistant(),
            },
            // Done is suppressed per spec
            AssistantMessageEvent::Done {
                reason: llm_client::StopReason::Stop,
                message: make_minimal_assistant(),
            },
        ];

        let provider = Arc::new(MockProvider { events });
        let (tx, mut rx) = mpsc::channel::<ServerEvent>(32);
        let state = Arc::new(Mutex::new(EventMapperState::new()));

        let streaming = StreamingProvider::new(provider, tx, state.clone());

        let ctx = llm_client::LlmContext {
            system_prompt: None,
            messages: vec![],
            tools: None,
        };

        let mut stream = streaming
            .stream("mock", ctx, StreamOptions::default(), CancellationToken::new())
            .await
            .unwrap();

        // Consume stream
        use futures::StreamExt;
        while let Some(_) = stream.next().await {}

        // Check forwarded events
        let mut received = Vec::new();
        while let Ok(event) = rx.try_recv() {
            received.push(event);
        }

        // Should have MessageStart + TextDelta (Done suppressed)
        assert!(received.iter().any(|e| matches!(e, ServerEvent::TextDelta { .. })));
        assert!(!received.iter().any(|e| matches!(e, ServerEvent::TurnEnd { .. })));
    }

    fn make_minimal_assistant() -> llm_client::AssistantMessage {
        llm_client::AssistantMessage {
            content: vec![],
            provider: "mock".into(),
            model: "mock".into(),
            api: llm_client::Api { provider: "mock".into(), model: "mock".into() },
            usage: llm_client::Usage::default(),
            stop_reason: llm_client::StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        }
    }
}
```

- [ ] **Step 6: Verify and commit**

```bash
cargo test -p tui streaming_provider
```

Expected: Test passes — `TextDelta` forwarded, `Done` suppressed.

```bash
git add crates/tui/src/backend/streaming_provider.rs crates/tui/src/backend/mod.rs
git commit -m "feat(tui): add StreamingProvider — LlmProvider wrapper that forwards events to TUI"
```

---

## Phase 4: LocalBackend

#### Task 4.1: Define `Backend` trait (in `backend/mod.rs`)

**Files:**
- Modify: `crates/tui/src/backend/mod.rs` (add trait + `pub mod local;`)

**Steps:**

- [ ] **Step 1: Write `Backend` trait (append to `backend/mod.rs`)**

```rust
// At the top of backend/mod.rs (above existing mod declarations):
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::client::model::{ServerEvent, SessionInfo};

#[async_trait]
pub trait Backend: Send + Sync {
    /// Create a new session. Returns session metadata.
    async fn create_session(
        &self,
        title: Option<&str>,
        model: &str,
    ) -> Result<SessionInfo, String>;

    /// List all known sessions.
    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, String>;

    /// Get metadata for a single session.
    async fn get_session(&self, id: &str) -> Result<SessionInfo, String>;

    /// Delete a session and its history.
    async fn delete_session(&self, id: &str) -> Result<(), String>;

    /// Clear the message history for a session.
    async fn clear_session(&self, id: &str) -> Result<(), String>;

    /// Send a user message to the session.
    ///
    /// The caller creates the mpsc channel and passes the sender.
    /// Events are streamed to the receiver end in the TUI event loop.
    /// This matches the existing SSE pattern where `submit_input`
    /// creates the channel and passes the tx to the transport layer.
    async fn send_message(
        &self,
        session_id: &str,
        content: &str,
        tx: mpsc::Sender<ServerEvent>,
    ) -> Result<(), String>;

    /// Interrupt an in-flight message stream.
    async fn interrupt(&self, session_id: &str) -> Result<(), String>;
}
```

- [ ] **Step 2: Update `backend/mod.rs` module declarations**

The final `backend/mod.rs` should have:
```rust
pub mod event_mapper;
pub mod streaming_provider;
pub mod local;
```
(Plus the `Backend` trait above them.)

- [ ] **Step 3: Verify**

```bash
cargo check -p tui
```

#### Task 4.2: Implement `LocalBackend`

**Files:**
- Create: `crates/tui/src/backend/local.rs`

**Steps:**

- [ ] **Step 1: Define `SessionData` and `LocalBackend` struct**

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

use agent_core::{
    AgentError, AgentLoop, HookDispatcher,
    types::AgentMessage,
};
use llm_client::{Content, LlmProvider};

use crate::backend::Backend;
use crate::backend::event_mapper::EventMapperState;
use crate::backend::streaming_provider::StreamingProvider;
use crate::client::model::{ServerEvent, SessionInfo, UsageInfo};

struct SessionData {
    info: SessionInfo,
    messages: Vec<AgentMessage>,
}

pub struct LocalBackend {
    sessions: Arc<tokio::sync::Mutex<HashMap<String, SessionData>>>,
    /// Active cancel tokens, keyed by session_id.  Stored separately from
    /// sessions so `interrupt()` works even when session data is temporarily
    /// removed from the map during `send_message()`.
    cancel_tokens: Mutex<HashMap<String, CancellationToken>>,
    provider: Arc<dyn LlmProvider>,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    system_prompt: String,
    context_window: u64,
    model: Mutex<String>,
}
```

- [ ] **Step 2: Implement constructor**

```rust
impl LocalBackend {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        system_prompt: String,
        model: String,
        context_window: u64,
    ) -> Self {
        Self {
            sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            cancel_tokens: Mutex::new(HashMap::new()),
            provider,
            hook_dispatcher,
            system_prompt,
            context_window,
            model: Mutex::new(model),
        }
    }
}
```

- [ ] **Step 3: Implement `create_session`, `list_sessions`, `get_session`, `delete_session`, `clear_session`**

```rust
#[async_trait]
impl Backend for LocalBackend {
    async fn create_session(
        &self,
        title: Option<&str>,
        model: &str,
    ) -> Result<SessionInfo, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let title = title.map(|t| t.to_string());
        let info = SessionInfo {
            id: id.clone(),
            title: title.clone(),
            model: model.to_string(),
            context_window: Some(self.context_window),
            created_at: None,
        };

        let session = SessionData {
            info: info.clone(),
            messages: Vec::new(),
        };

        let mut sessions = self.sessions.lock().await;
        sessions.insert(id.clone(), session);

        let mut tokens = self.cancel_tokens.lock().expect("mutex poisoned");
        tokens.insert(id.clone(), CancellationToken::new());

        info!(session_count = sessions.len(), "session created");
        Ok(info)
    }

    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, String> {
        let sessions = self.sessions.lock().await;
        Ok(sessions.values().map(|s| s.info.clone()).collect())
    }

    async fn get_session(&self, id: &str) -> Result<SessionInfo, String> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(id)
            .map(|s| s.info.clone())
            .ok_or_else(|| format!("session {} not found", id))
    }

    async fn delete_session(&self, id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().await;
        if sessions.remove(id).is_some() {
            info!(session_id = %id, "session deleted");
            Ok(())
        } else {
            Err(format!("session {} not found", id))
        }
    }

    async fn clear_session(&self, id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get_mut(id) {
            session.messages.clear();
            // Replace cancel token so any pending prompt is cancelled
            let mut tokens = self.cancel_tokens.lock().expect("mutex poisoned");
            if let Some(token) = tokens.get(id) {
                token.cancel();
            }
            tokens.insert(id.to_string(), CancellationToken::new());
            Ok(())
        } else {
            Err(format!("session {} not found", id))
        }
    }
}
```

- [ ] **Step 4: Implement `send_message` (new signature: takes tx, returns Result<(), String>)**

```rust
    async fn send_message(
        &self,
        session_id: &str,
        content: &str,
        tx: mpsc::Sender<ServerEvent>,
    ) -> Result<(), String> {
        let model = self.model.lock().expect("mutex poisoned").clone();

        // Create a fresh cancel token for this prompt.
        // Store it in cancel_tokens so interrupt() works even while
        // session data is temporarily removed from the sessions map.
        let cancel_token = {
            let mut tokens = self.cancel_tokens.lock().expect("mutex poisoned");
            let token = CancellationToken::new();
            tokens.insert(session_id.to_string(), token.clone());
            token
        };

        // Remove session data (re-insert after prompt to avoid holding
        // the tokio::sync::Mutex across an .await point).
        let mut session_data = {
            let mut sessions = self.sessions.lock().await;
            sessions
                .remove(session_id)
                .ok_or_else(|| format!("session {} not found", session_id))?
        };

        // Append user message
        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text {
                text: content.to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });
        session_data.messages.push(user_msg);

        // Build StreamingProvider + AgentLoop
        let state = Arc::new(Mutex::new(EventMapperState::new()));
        let streaming_provider = Arc::new(StreamingProvider::new(
            self.provider.clone(),
            tx.clone(),
            state.clone(),
        ));

        let loop_ = AgentLoop::new(
            "local".to_string(),           // tenant_id
            session_id.to_string(),        // session_id
            model,
            streaming_provider.clone(),
            self.hook_dispatcher.clone(),
            vec![],                        // no tools in MVP
        );

        let msgs = session_data.messages.clone();
        let system_prompt = self.system_prompt.clone();
        let sessions = self.sessions.clone();
        let sid = session_id.to_string();

        // Spawn the agent loop
        tokio::spawn(async move {
            let result = loop_
                .run(Some(system_prompt), msgs, cancel_token.child_token())
                .await;

            match result {
                Ok(new_msgs) => {
                    // Scan for tool results → emit ToolCallDone
                    let tool_names = streaming_provider.tool_names();
                    for msg in &new_msgs {
                        if let AgentMessage::ToolResult(ref tr) = msg {
                            let _name = tool_names
                                .get(&tr.tool_call_id)
                                .cloned()
                                .unwrap_or_else(|| tr.tool_name.clone());
                            let _ = tx.send(ServerEvent::ToolCallDone {
                                call_id: tr.tool_call_id.clone(),
                                result: tr.content.first().and_then(|c| match c {
                                    Content::Text { text, .. } => Some(text.clone()),
                                    _ => None,
                                }),
                                is_error: tr.is_error,
                            }).await;
                        }
                    }

                    // Emit TurnEnd
                    let usage = new_msgs.last().and_then(|m| {
                        if let AgentMessage::Assistant(ref a) = m {
                            Some(UsageInfo {
                                input_tokens: a.usage.input_tokens,
                                output_tokens: a.usage.output_tokens,
                            })
                        } else {
                            None
                        }
                    });

                    let _ = tx.send(ServerEvent::TurnEnd {
                        stop_reason: "stop".to_string(),
                        usage,
                    }).await;

                    // Re-insert session data with updated messages
                    session_data.messages.extend(new_msgs);
                    let mut sessions = sessions.lock().await;
                    sessions.insert(sid, session_data);
                }
                Err(e) => {
                    let _ = tx.send(ServerEvent::Error {
                        code: "agent_error".to_string(),
                        message: format!("{}", e),
                    }).await;

                    // Re-insert session data on error
                    let mut sessions = sessions.lock().await;
                    sessions.insert(sid, session_data);
                }
            }
        });

        Ok(())
    }
```

- [ ] **Step 5: Implement `interrupt`**

```rust
    async fn interrupt(&self, session_id: &str) -> Result<(), String> {
        let mut tokens = self.cancel_tokens.lock().expect("mutex poisoned");
        if let Some(token) = tokens.get(session_id) {
            token.cancel();
            info!(session_id = %session_id, "session interrupted");
            Ok(())
        } else {
            Err(format!("session {} not found", session_id))
        }
    }
```

**How this works with `send_message`:** In `send_message`, a fresh `CancellationToken` is inserted into `cancel_tokens` before removing session data from `sessions`. This means `interrupt()` works even while the session is temporarily absent from the sessions map during a prompt. After the prompt completes, the token reference is dropped and garbage-collected.

- [ ] **Step 6: Verify**

```rust
pub mod event_mapper;
pub mod streaming_provider;
pub mod local;
```

- [ ] **Step 7: Verify**

```bash
cargo check -p tui
```

Fix any compilation errors. Expected: `LocalBackend` types check successfully. `Backend` trait is defined.

---

## Phase 5: Configuration Extension

#### Task 5.1: Add `LlmConfig` and extend `CliArgs`

**Files:**
- Modify: `crates/tui/src/config.rs`

**Steps:**

- [ ] **Step 1: Add `LlmConfig` struct**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// LLM provider name (anthropic, openai, google, mistral)
    #[serde(default = "default_llm_provider")]
    pub provider: String,

    /// Model name
    #[serde(default = "default_llm_model")]
    pub model: String,
}

fn default_llm_provider() -> String {
    "anthropic".to_string()
}

fn default_llm_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_llm_provider(),
            model: default_llm_model(),
        }
    }
}
```

- [ ] **Step 2: Update `Config` struct — make server optional, add llm**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Server config. None → local mode.
    #[serde(default)]
    pub server: Option<ServerConfig>,

    pub auth: AuthConfig,

    #[serde(default)]
    pub llm: LlmConfig,

    #[serde(default)]
    pub ui: UiConfig,

    #[serde(default)]
    pub keys: KeysConfig,

    /// Force local mode (from --local CLI flag). Not persisted to config file.
    #[serde(skip)]
    pub force_local: bool,
}
```

- [ ] **Step 3: Update `ServerConfig` serialization**

Make `ServerConfig` derive `Serialize, Deserialize` (already does). Ensure `url` is required when the struct is present.

- [ ] **Step 4: Extend `CliArgs`**

```rust
#[derive(Parser, Debug)]
#[command(name = "pandaria-tui")]
pub struct CliArgs {
    /// Server URL (remote mode). If omitted, runs in local mode.
    #[arg(long, env = "PANDARIA_URL")]
    pub url: Option<String>,

    /// Auth token (API key in local mode, Bearer token in remote mode)
    #[arg(long, env = "PANDARIA_TOKEN")]
    pub token: Option<String>,

    /// Force local mode even if --url is set
    #[arg(long, default_value_t = false)]
    pub local: bool,

    /// LLM provider for local mode (anthropic, openai, google, mistral)
    #[arg(long, env = "PANDARIA_PROVIDER")]
    pub provider: Option<String>,

    /// Model name for local mode
    #[arg(long, env = "PANDARIA_MODEL")]
    pub model: Option<String>,

    /// Theme name
    #[arg(long)]
    pub theme: Option<String>,

    /// Config file path
    #[arg(long, env = "PANDARIA_CONFIG")]
    pub config: Option<String>,
}
```

- [ ] **Step 5: Update `Config::load()`**

Merge CLI args into config:

```rust
impl Config {
    pub fn load(cli: CliArgs) -> Result<Self, Box<dyn std::error::Error>> {
        // Load from config file first
        let mut config = Self::load_from_file(cli.config.as_deref())?;

        // CLI overrides env over config file
        if let Some(url) = cli.url {
            config.server = Some(ServerConfig {
                url,
                timeout_secs: config.server.as_ref()
                    .map(|s| s.timeout_secs)
                    .unwrap_or(30),
            });
        }
        if let Some(token) = cli.token {
            config.auth.token = Some(token);
        }
        if let Some(provider) = cli.provider {
            config.llm.provider = provider;
        }
        if let Some(model) = cli.model {
            config.llm.model = model;
        }
        config.force_local = cli.local;

        Ok(config)
    }
}
```

- [ ] **Step 6: Add mode detection helper**

```rust
impl Config {
    /// Returns true if the TUI should run in local mode.
    pub fn is_local_mode(&self) -> bool {
        // --local flag forces local mode even if server URL is configured
        // Otherwise: local mode when no server URL is configured
        self.force_local || self.server.is_none()
    }

    /// Returns the effective provider name.
    pub fn provider(&self) -> &str {
        &self.llm.provider
    }

    /// Returns the effective model name.
    pub fn model(&self) -> &str {
        &self.llm.model
    }
}
```

**Note:** Adjust `Config::load_from_file` to handle `ServerConfig` as `Option`. If the `[server]` section is absent from the TOML file, `config.server` should be `None`.

- [ ] **Step 7: Verify**

```bash
cargo check -p tui
cargo test -p tui config
```

Expected: Config compiles with new fields. Existing config tests still pass (if any fail due to struct shape changes, update test fixtures).

---

## Phase 6: App Refactor

#### Task 6.1: Replace `RestClient` with `Box<dyn Backend>`

**Files:**
- Modify: `crates/tui/src/app.rs`

**Steps:**

- [ ] **Step 1: Update `App` struct**

```rust
use crate::backend::Backend;

pub struct App {
    pub state: AppState,
    pub data: crate::state::State,
    pub config: Config,
    pub theme: Theme,
    pub backend: Box<dyn Backend>,       // ← replaces rest: RestClient
    pub input: InputBar,
    pub spinner: SpinnerWidget,
    pub overlays: OverlayStack,
    pub paste_store: PasteStore,
    pub context_window: Option<u64>,
    pub input_tokens: u64,
    pub server_rx: Option<mpsc::Receiver<ServerEvent>>,
    pub scroll_offset: usize,
    pub user_scrolled_up: bool,
    pub running: bool,
}
```

Remove fields: `rest: RestClient`, `reqwest_client: reqwest::Client`.

Remove imports: `crate::client::rest::RestClient`, `reqwest::Client`.

- [ ] **Step 2: Update `App::new()`**

```rust
impl App {
    pub fn new(
        config: Config,
        backend: Box<dyn Backend>,
        session_id: String,
        session_info: crate::client::model::SessionInfo,
    ) -> Self {
        let data = crate::state::State::new(session_id, session_info);
        let context_window = data.active_session().info.context_window;
        Self {
            state: AppState::Connected,
            data,
            config,
            theme: Theme::default(),
            backend,
            input: InputBar::new(),
            spinner: SpinnerWidget::new(),
            overlays: OverlayStack::new(),
            paste_store: PasteStore::new(),
            context_window,
            input_tokens: 0,
            server_rx: None,
            scroll_offset: 0,
            user_scrolled_up: false,
            running: true,
        }
    }
}
```

- [ ] **Step 3: Update `handle_overlay_confirm` commands**

`Box<dyn Backend>` is replaced by `Arc<dyn Backend>` to allow cloning into spawned tasks.

```rust
fn handle_overlay_confirm(&mut self, value: String) {
    if let Some(cmd) = Command::parse(&value) {
        match cmd {
            Command::Quit => self.running = false,
            Command::Help => {
                self.overlays.push(Box::new(crate::overlays::help::HelpOverlay::new()));
            }
            Command::Clear => {
                self.data.active_session_mut().messages.clear();
                let backend = self.backend.clone();
                let sid = self.data.active_session.clone();
                tokio::spawn(async move {
                    let _ = backend.clear_session(&sid).await;
                });
            }
            Command::NewSession { title } => {
                let backend = self.backend.clone();
                let model = self.config.llm.model.clone();
                tokio::spawn(async move {
                    match backend.create_session(title.as_deref(), &model).await {
                        Ok(_info) => {
                            // TODO: emit event to update State.
                            // For MVP: next session list refresh picks it up.
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "create_session failed");
                        }
                    }
                });
            }
            Command::SwitchSession { id } => {
                if self.data.sessions.contains_key(&id) {
                    self.data.active_session = id;
                }
            }
            Command::ListSessions => {
                let sessions: Vec<_> = self.data.sessions.iter()
                    .map(|(id, s)| (id.clone(), s.info.title.clone()
                        .unwrap_or_else(|| id.chars().take(8).collect())))
                    .collect();
                self.overlays.push(Box::new(
                    crate::overlays::session_list::SessionListOverlay::new(sessions),
                ));
            }
            Command::SelectModel { id } => {
                if let Some(model_id) = id {
                    let session = self.data.active_session_mut();
                    session.info.model = model_id;
                } else {
                    let models = vec!["gpt-4o".to_string(), "claude-sonnet-4-20250514".to_string()];
                    self.overlays.push(Box::new(
                        crate::overlays::model_selector::ModelSelector::new(models),
                    ));
                }
            }
            Command::Connect { .. } => {
                // In local mode, this is a no-op.
            }
            Command::Auth { token } => {
                self.config.auth.token = Some(token);
            }
            Command::Tokens => { /* displayed in StatusBar */ }
        }
    }
}
```

```rust
fn submit_input(&mut self) {
    let text = self.input.take_text();
    if text.trim().is_empty() {
        return;
    }

    if text.starts_with('/') {
        self.handle_overlay_confirm(text);
        return;
    }

    let text = self.paste_store.expand(&text);

    let msg = RenderedMessage {
        role: MessageRole::User,
        blocks: vec![MessageBlock::Text(vec![ratatui::text::Line::from(text.clone())])],
        timestamp: std::time::SystemTime::now(),
        status: MessageStatus::Complete,
    };
    self.data.active_session_mut().messages.push(msg);

    let backend = self.backend.clone();
    let sid = self.data.active_session.clone();
    let content = text.clone();
    let (tx, rx) = mpsc::channel::<ServerEvent>(32);

    tokio::spawn(async move {
        if let Err(e) = backend.send_message(&sid, &content, tx).await {
            tracing::error!(error = %e, "send_message failed");
        }
    });

    self.server_rx = Some(rx);

    let assistant_msg = RenderedMessage {
        role: MessageRole::Assistant,
        blocks: Vec::new(),
        timestamp: std::time::SystemTime::now(),
        status: MessageStatus::Streaming,
    };
    self.data.active_session_mut().messages.push(assistant_msg);
    self.data.active_session_mut().streaming = Some(StreamingBuffer {
        text_content: String::new(),
        thinking_content: String::new(),
        pending_tool_calls: Vec::new(),
        tool_arg_buffers: HashMap::new(),
    });

    self.state = AppState::Busy;
    self.user_scrolled_up = false;
}
```

**Note:** Also update `interrupt` in the Esc handler:

```rust
KeyCode::Esc => {
    if self.state == AppState::Busy {
        let backend = self.backend.clone();
        let sid = self.data.active_session.clone();
        tokio::spawn(async move {
            let _ = backend.interrupt(&sid).await;
        });
        // ...
    }
}
```

Remove the `Ctrl+C` handler that clones `rest` and calls `rest.interrupt()`. Replace with `backend.interrupt()`.

- [ ] **Step 5: Handle `handle_overlay_confirm` commands**

```rust
fn handle_overlay_confirm(&mut self, value: String) {
    if let Some(cmd) = Command::parse(&value) {
        match cmd {
            Command::Quit => self.running = false,
            Command::Help => {
                self.overlays.push(Box::new(crate::overlays::help::HelpOverlay::new()));
            }
            Command::Clear => {
                self.data.active_session_mut().messages.clear();
                let backend = self.backend.clone();
                let sid = self.data.active_session.clone();
                tokio::spawn(async move {
                    let _ = backend.clear_session(&sid).await;
                });
            }
            Command::NewSession { title } => {
                let backend = self.backend.clone();
                let model = self.config.llm.model.clone();
                tokio::spawn(async move {
                    match backend.create_session(title.as_deref(), &model).await {
                        Ok(_info) => {
                            // TODO: emit event to update State
                            // For MVP: next session list refresh picks it up
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "create_session failed");
                        }
                    }
                });
            }
            Command::SwitchSession { id } => {
                if self.data.sessions.contains_key(&id) {
                    self.data.active_session = id;
                }
            }
            Command::ListSessions => {
                let sessions: Vec<_> = self.data.sessions.iter()
                    .map(|(id, s)| (id.clone(), s.info.title.clone()
                        .unwrap_or_else(|| id.chars().take(8).collect())))
                    .collect();
                self.overlays.push(Box::new(
                    crate::overlays::session_list::SessionListOverlay::new(sessions),
                ));
            }
            Command::SelectModel { id } => {
                if let Some(model_id) = id {
                    let session = self.data.active_session_mut();
                    session.info.model = model_id;
                } else {
                    let models = vec!["gpt-4o".to_string(), "claude-sonnet-4-20250514".to_string()];
                    self.overlays.push(Box::new(
                        crate::overlays::model_selector::ModelSelector::new(models),
                    ));
                }
            }
            Command::Connect { .. } => {
                // In local mode, this is a no-op.
            }
            Command::Auth { token } => {
                self.config.auth.token = Some(token);
            }
            Command::Tokens => { /* displayed in StatusBar */ }
        }
    }
}
```
The `_url` binding is unused in local mode. Use `..` in the match arm or `_url` to suppress the warning.

- [ ] **Step 4: Implement `submit_input` with new `Arc<dyn Backend>` + `tx` parameter pattern**

```rust
fn submit_input(&mut self) {
    let text = self.input.take_text();
    if text.trim().is_empty() {
        return;
    }

    if text.starts_with('/') {
        self.handle_overlay_confirm(text);
        return;
    }

    let text = self.paste_store.expand(&text);

    let msg = RenderedMessage {
        role: MessageRole::User,
        blocks: vec![MessageBlock::Text(vec![ratatui::text::Line::from(text.clone())])],
        timestamp: std::time::SystemTime::now(),
        status: MessageStatus::Complete,
    };
    self.data.active_session_mut().messages.push(msg);

    let backend = self.backend.clone();
    let sid = self.data.active_session.clone();
    let content = text.clone();
    let (tx, rx) = mpsc::channel::<ServerEvent>(32);

    tokio::spawn(async move {
        if let Err(e) = backend.send_message(&sid, &content, tx).await {
            tracing::error!(error = %e, "send_message failed");
        }
    });

    self.server_rx = Some(rx);

    let assistant_msg = RenderedMessage {
        role: MessageRole::Assistant,
        blocks: Vec::new(),
        timestamp: std::time::SystemTime::now(),
        status: MessageStatus::Streaming,
    };
    self.data.active_session_mut().messages.push(assistant_msg);
    self.data.active_session_mut().streaming = Some(StreamingBuffer {
        text_content: String::new(),
        thinking_content: String::new(),
        pending_tool_calls: Vec::new(),
        tool_arg_buffers: HashMap::new(),
    });

    self.state = AppState::Busy;
    self.user_scrolled_up = false;
}
```

**Note:** `send_message` takes `tx: mpsc::Sender<ServerEvent>` — the caller owns the channel creation, and the backend writes events to it. `self.server_rx = Some(rx)` gives the TUI event loop the receiver end. This is the same pattern used by the original SSE code path.

- [ ] **Step 5: Update interrupt handlers (Esc and Ctrl+C)**

```rust
KeyCode::Esc => {
    if self.state == AppState::Busy {
        let backend = self.backend.clone();
        let sid = self.data.active_session.clone();
        tokio::spawn(async move {
            let _ = backend.interrupt(&sid).await;
        });
        if let Some(last) = self.data.active_session_mut().messages.last_mut() {
            last.status = MessageStatus::Aborted;
        }
        self.state = AppState::Connected;
    } else {
        self.input.clear();
    }
}
```

Replace the Ctrl+C handler that clones `rest` with:

```rust
'c' => {
    if self.state == AppState::Busy {
        let backend = self.backend.clone();
        let sid = self.data.active_session.clone();
        tokio::spawn(async move {
            let _ = backend.interrupt(&sid).await;
        });
        if let Some(last) = self.data.active_session_mut().messages.last_mut() {
            last.status = MessageStatus::Aborted;
        }
        self.state = AppState::Connected;
    } else {
        self.running = false;
    }
}
```

- [ ] **Step 6: Remove `rest` and `reqwest_client` from all remaining code paths**

Search for `self.rest` and `self.reqwest_client` in `app.rs` and remove/replace them.

- [ ] **Step 7: Update `render_ui`**

The `render_ui` method passes `self.input` to `input.render()`. No changes needed since `InputBar` didn't change.

- [ ] **Step 8: Verify**

```bash
cargo check -p tui
```

Expected: Compilation errors due to type changes in `main.rs` (still constructs `RestClient`). Fix in Phase 7.

---

## Phase 7: Main Entry Point

#### Task 7.1: Provider construction, mode selection, `LocalBackend` init

**Files:**
- Modify: `crates/tui/src/main.rs`

**Steps:**

- [ ] **Step 1: Remove old server-dependent startup**

Remove:
```rust
let rest = RestClient::new(&config.server);
let sessions = rest.list_sessions(&token).await...;
let session_info = if let Some(first) = sessions.into_iter().next() { ... };
```

- [ ] **Step 2: Add provider construction**

```rust
use secrecy::SecretString;
use std::sync::Arc;

fn build_provider(
    provider_name: &str,
    api_key: SecretString,
) -> Result<Arc<dyn llm_client::LlmProvider>, String> {
    match provider_name {
        "anthropic" => {
            let p = llm_client::providers::anthropic::AnthropicProvider::new(
                Some(api_key),
            );
            Ok(Arc::new(p))
        }
        "openai" => {
            let p = llm_client::providers::openai::OpenAiProvider::new(
                Some(api_key),
            );
            Ok(Arc::new(p))
        }
        "google" => {
            let p = llm_client::providers::google::GoogleProvider::new(
                Some(api_key),
            );
            Ok(Arc::new(p))
        }
        "mistral" => {
            let p = llm_client::MistralProvider::new(Some(api_key));
            Ok(Arc::new(p))
        }
        _ => Err(format!("Unknown provider: {}", provider_name)),
    }
}
```

- [ ] **Step 3: Construct `HookDispatcher`**

```rust
use agent_core::HookDispatcher;

struct NoopDispatcher;
#[async_trait::async_trait]
impl HookDispatcher for NoopDispatcher {}
```

- [ ] **Step 4: Construct `LocalBackend` and `App`**

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("pandaria_tui=info")
        .init();

    let cli = CliArgs::parse();
    let config = Config::load(cli)?;

    // Validate: local mode requires token
    let token = config.auth.token.clone()
        .ok_or("No token. Set PANDARIA_TOKEN env var, --token flag, or config file.")?;

    if config.is_local_mode() {
        // LOCAL MODE
        let api_key = SecretString::new(token.into());
        let provider = build_provider(config.provider(), api_key)
            .map_err(|e| format!("Failed to create provider: {}", e))?;
        let dispatcher = Arc::new(NoopDispatcher);

        let backend = Arc::new(LocalBackend::new(
            provider,
            dispatcher,
            "You are a helpful assistant.".to_string(),
            config.model().to_string(),
            128_000, // context window
        ));

        // Create initial session
        let session_info = backend.create_session(None, config.model()).await
            .map_err(|e| format!("Failed to create session: {}", e))?;

        // Terminal setup
        let mut stdout = io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        enable_raw_mode()?;
        let backend_term = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend_term)?;

        let mut app = App::new(config, backend, session_info.id.clone(), session_info);
        // ... event loop unchanged ...
    } else {
        // REMOTE MODE (deferred — error for now)
        return Err("Remote mode not yet implemented. Use --local or omit --url.".into());
    }

    // ... event loop (unchanged) ...
}
```

- [ ] **Step 5: Update imports in main.rs**

Remove:
```rust
use tui::client::rest::RestClient;
```

Add:
```rust
use secrecy::SecretString;
use std::sync::Arc;
use tui::backend::local::LocalBackend;
use agent_core::HookDispatcher;
```

- [ ] **Step 6: The event loop is unchanged**

The `tokio::select!` loop stays the same — it reads from `app.server_rx` (which is populated by `submit_input`) and `crossterm::EventStream`.

- [ ] **Step 7: Verify compilation**

```bash
cargo check -p tui
```

Fix any compilation errors. Expected: Binary compiles.

---

## Phase 8: Integration & Testing

#### Task 8.1: Run existing tests

- [ ] **Step 1: Run all tui tests**

```bash
cargo test -p tui
```

Check that existing tests (command parser, markdown, paste, model serde) still pass. New backend tests pass.

#### Task 8.2: Integration test with EchoProvider

- [ ] **Step 1: Write integration test**

Create `crates/tui/tests/local_mode.rs` (or add to existing `tests/integration.rs`):

```rust
use std::sync::Arc;
use secrecy::SecretString;
use tui::backend::local::LocalBackend;
use tui::backend::Backend;
use llm_client::providers::anthropic::AnthropicProvider;

#[tokio::test]
async fn test_local_mode_create_and_send() {
    // Use a mock provider from agent-core's test suite
    // (EchoProvider returns "response" for any input)
    // Or test just the session management without LLM:

    // Create backend
    let backend = LocalBackend::new(
        Arc::new(MockEchoProvider),
        Arc::new(NoopDispatcher),
        "You are helpful.".into(),
        "echo".into(),
        128_000,
    );

    // Create session
    let info = backend.create_session(Some("test"), "echo").await.unwrap();
    assert!(!info.id.is_empty());
    assert_eq!(info.title, Some("test".to_string()));

    // List sessions
    let sessions = backend.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);

    // Send message
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ServerEvent>(32);
    backend.send_message(&info.id, "hello", tx).await.unwrap();

    // Collect events
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        let is_terminal = matches!(&event,
            ServerEvent::TurnEnd { .. } | ServerEvent::Error { .. }
        );
        events.push(event);
        if is_terminal {
            break;
        }
    }

    // Verify TurnEnd received
    assert!(events.iter().any(|e| matches!(e, ServerEvent::TurnEnd { .. })));

    // Clear session
    backend.clear_session(&info.id).await.unwrap();
}
```

#### Task 8.3: Manual verification

- [ ] **Step 1: Test with real Anthropic API key**

```bash
PANDARIA_TOKEN=sk-ant-... PANDARIA_PROVIDER=anthropic cargo run -p tui
```

Expected: TUI starts in alternate screen. Type a message and see streaming LLM response.

- [ ] **Step 2: Test slash commands**

```
/new test session    → creates new session
/switch <id>         → switches to different session
/model               → opens model selector
/clear               → clears messages
/help                → shows help overlay
/quit                → exits
```

- [ ] **Step 3: Test interrupt**

Send a long-answer query. Press Esc while streaming. Verify the assistant message shows "Aborted" state.

- [ ] **Step 4: Test with OpenAI**

```bash
PANDARIA_TOKEN=sk-... PANDARIA_PROVIDER=openai PANDARIA_MODEL=gpt-4o cargo run -p tui
```

---

## Verification Checklist

Before declaring Phase 8 complete:

- [ ] `cargo check -p tui` passes
- [ ] `cargo test -p tui` passes (all existing + new backend tests)
- [ ] `cargo build -p tui` produces a binary
- [ ] `PANDARIA_TOKEN=... cargo run -p tui` starts the TUI
- [ ] Typing a message produces streaming LLM response
- [ ] `/new` creates a new session
- [ ] `/clear` clears messages
- [ ] `/model` opens overlay
- [ ] `Esc` interrupts streaming
- [ ] `Ctrl+C` exits when idle
- [ ] No panics or unwrap failures during normal use

---

## File Map (Summary)

### New Files
| File | Lines (est.) | Phase |
|---|---|---|
| `crates/tui/src/backend/mod.rs` | ~60 | 2,4 | (created Phase 2, Backend trait added Phase 4) |
| `crates/tui/src/backend/event_mapper.rs` | ~150 | 2 |
| `crates/tui/src/backend/streaming_provider.rs` | ~100 | 3 |
| `crates/tui/src/backend/local.rs` | ~250 | 4 |
| `crates/tui/tests/local_mode.rs` | ~80 | 8 |

### Modified Files
| File | Phase | Change Summary |
|---|---|---|
| `crates/tui/Cargo.toml` | 1 | +agent-core, +llm-client, +secrecy, +uuid |
| `crates/tui/src/lib.rs` | 1 | +`pub mod backend;` |
| `crates/tui/src/config.rs` | 5 | +`LlmConfig`, `ServerConfig` optional, `force_local`, new CLI args |
| `crates/tui/src/app.rs` | 6 | `RestClient` → `Arc<dyn Backend>`, rewrite `submit_input`, `handle_overlay_confirm`, interrupt |
| `crates/tui/src/main.rs` | 7 | Provider construction, backend init, mode selection |

### Unchanged Files
`state.rs`, `command.rs`, `markdown.rs`, `paste.rs`, `client/*`, `ui/theme.rs`, `widgets/*`, `overlays/*`
