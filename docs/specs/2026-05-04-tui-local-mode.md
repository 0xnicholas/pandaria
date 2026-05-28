# Pandaria TUI — Local Mode Integration Spec

**Date:** 2026-05-04
**Status:** Completed ✅ — TUI local mode delivered
**Reference:** `docs/specs/2026-05-02-tui-design.md`, `docs/specs/2026-05-02-agent-core.md`, `docs/specs/2026-05-02-ai-provider.md`

---

## 1. Purpose

Integrate `agent-core` and `ai-provider` directly into the TUI crate so that the TUI works as a **self-contained standalone CLI tool** without requiring a running pandaria server.

The TUI currently communicates with a server via SSE + HTTP REST, but the server binary does not exist yet. This spec defines a **local mode** that embeds the agent runtime in-process, while preserving the existing HTTP/SSE client path for future server mode via a `Backend` trait abstraction.

**Goals:**
- Make all TUI slash commands (`/new`, `/switch`, `/model`, `/clear`, etc.) functional
- Stream LLM responses to the TUI in real-time (text, thinking, tool calls)
- Zero changes to `agent-core` and `ai-provider` crates — integration happens entirely in the TUI crate
- Preserve the existing `ServerEvent`-based UI rendering path — no duplicate rendering code

**Non-goals:**
- Building the pandaria server binary (deferred)
- Multi-tenant isolation (single-tenant local mode only)
- Session persistence (in-memory only)

---

## 2. Architecture Overview

```
┌─ TUI Event Loop (main.rs) ──────────────────────────────────┐
│  tokio::select! {                                            │
│    crossterm keys  → App::handle_key_event()                 │
│    mpsc::Receiver<ServerEvent> → App::handle_server_event()  │
│    spinner tick                                              │
│  }                                                           │
└───────────────────────┬─────────────────────────────────────┘
                        │ mpsc::Receiver<ServerEvent>
                        │
┌─ App ───────────────────────────────────────────────────────┐
│  backend: Box<dyn Backend>   ← replaces RestClient           │
│  submit_input() → backend.send_message()                     │
│  /new, /switch, /model → backend methods                     │
└───────────────────────┬─────────────────────────────────────┘
                        │
            ┌───────────┴───────────┐
            │                       │
     ┌──────▼──────┐        ┌──────▼──────┐
     │ LocalBackend│        │ HttpBackend │ (future)
     │             │        │ (RestClient)│
     │ SessionData │        └─────────────┘
     │ Streaming   │
     │ Provider    │
     └──────┬──────┘
            │
     ┌──────▼──────────────────────┐
     │ agent-core::AgentLoop       │
     │   └── run(messages,         │
     │          provider) → stream │
     │         ↓                   │
     │ StreamingProvider           │
     │   └── intercept events →    │
     │       mpsc::Sender          │
     └─────────────────────────────┘
```

The `Backend` trait abstracts session operations. `LocalBackend` implements it using `agent-core` directly. The existing `App::handle_server_event()` and `handle_overlay_confirm()` methods work unchanged — they receive the same `ServerEvent` types through the same `mpsc` channel.

---

## 3. Backend Trait

Defined in `crates/tui/src/backend.rs`.

```rust
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
    /// Streaming events (text deltas, tool calls, turn end, errors)
    /// are written to `tx`. The caller reads from the receiver end
    /// in the TUI event loop.
    ///
    /// This matches the existing SSE client pattern where `submit_input`
    /// creates the channel and passes the sender to the transport layer.
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

**Design rationale for `mpsc::Receiver` return type:** The TUI's existing event loop uses `self.server_rx: Option<mpsc::Receiver<ServerEvent>>`. By returning a `Receiver` from `send_message`, `LocalBackend` plugs directly into this existing channel without changing `App`'s event-loop code or `handle_server_event()`.

---

## 4. LocalBackend

Defined in `crates/tui/src/backend/local.rs`.

### 4.1 Structure

```rust
pub struct LocalBackend {
    /// Active sessions, keyed by session ID. Each session holds message history
    /// and metadata. No `SessionActor` — the stateless `AgentLoop` is used
    /// directly in `send_message()` to avoid a circular construction dependency
    /// (AgentLoop takes the provider at call time, not construction time).
    sessions: Arc<tokio::sync::Mutex<HashMap<String, SessionData>>>,

    /// LLM provider shared across all sessions.
    provider: Arc<dyn LlmProvider>,

    /// Hook dispatcher (noop for local mode, but extensible later).
    hook_dispatcher: Arc<dyn HookDispatcher>,

    /// Default system prompt for new sessions.
    system_prompt: String,

    /// Default context window for new sessions.
    context_window: u64,

    /// Current model name.
    model: Mutex<String>,
}

struct SessionData {
    info: SessionInfo,
    /// Full message history (user, assistant, tool result). Passed to
    /// AgentLoop::run() on each prompt; new messages are appended after.
    messages: Vec<AgentMessage>,
    /// CancellationToken for aborting the in-flight prompt.
    cancel_token: CancellationToken,
}
```

**Design decisions:**

- **`Arc<tokio::sync::Mutex<...>>`** — Tokio's async-aware mutex, safe to hold across `.await` points. The `send_message` flow removes the session from the map (so the lock is released before the prompt), runs AgentLoop, then re-inserts — no lock is held across the LLM call.
- **`SessionData`, not `SessionActor`** — `SessionActor` stores an `Arc<dyn LlmProvider>` at construction time, which creates a circular dependency with `StreamingProvider` (see §4.2). The stateless `AgentLoop::run()` takes messages + provider as parameters, avoiding this. Steer/follow-up queues and auto-compaction are sacrificed for MV simplicity.
- **Shared provider** — All sessions use the same `LlmProvider` instance. This avoids re-creating HTTP connection pools per session.
- **Noop `HookDispatcher`** — Local mode uses `AllowAllDispatcher` (all hooks default to no-op). Extension hooks can be wired in later without changing the `Backend` trait.
- **`model: Mutex<String>`** — The current model name is shared across all sessions under `LocalBackend`. Per-session model selection is deferred (single-model usage is sufficient for local mode MVP).

**Important limitation — no tools:** Local mode MVP ships with zero registered tools (`AgentLoop` receives `vec![]`). All agent responses will be text-only. The tool call rendering paths in the TUI (ToolCallWidget, ToolCallState) are exercised by the StreamingProvider event mapping code but will never fire from actual LLM tool use. Built-in tools (file read/write, shell, etc.) are deferred to a follow-up work item.

### 4.2 `send_message` Implementation

```
LocalBackend::send_message(session_id, content, tx):
  1. Create fresh CancellationToken, store in cancel_tokens map
     ── (so interrupt() works even while session data is removed) ──
  2. Lock sessions, remove SessionData, drop lock
     ── (tokio::sync::MutexGuard released before any .await) ──
  3. Append user message to session.messages
  4. Build StreamingProvider(inner_provider, tx, state)
  5. Build AgentLoop(tenant_id, session_id, model,
                     streaming_provider, hook_dispatcher, vec![])
  6. Spawn tokio task:
     a. Call agent_loop.run(system_prompt, messages, cancel_token.child_token())
     b. The loop streams via StreamingProvider → events land in tx
     c. After loop returns Vec<AgentMessage>:
        - Append returned messages to session.messages
        - Scan for ToolResult messages → emit ToolCallDone events via tx
        - Emit single TurnEnd { stop_reason, usage } via tx
     d. If error: emit Error event via tx
     e. Re-lock sessions, re-insert session data, drop lock
     f. Drop tx (signals end of stream to receiver)
  7. Return Ok(()) — events are delivered asynchronously via tx
```

**Key points:**

- **`tx` is provided by the caller** — The TUI's `submit_input()` creates the channel and passes the sender. This matches the existing SSE pattern where `sse::connect()` receives a pre-created `tx`. No channel forwarding hop.
- **`HashMap::remove` + spawn** — The session data is removed from the map before spawning, so the `tokio::sync::Mutex` guard is dropped immediately. No lock is held across the LLM call.
- **`cancel_tokens` for interruptability** — A fresh `CancellationToken` is stored in a separate `cancel_tokens` map (not inside `SessionData`). `interrupt()` cancels this token directly, regardless of whether the session is temporarily absent from the sessions map.
- **`AgentLoop`, not `SessionActor`** — `AgentLoop::run()` takes the provider as a parameter (no construction-time binding). This avoids circular construction between `StreamingProvider` and `SessionActor`.

### 4.3 `interrupt` Implementation

```
LocalBackend::interrupt(session_id):
  1. Lock sessions, get SessionData by session_id
  2. Call session.cancel_token.cancel()
  3. The CancellationToken propagates to the LLM stream,
     which returns LlmError::Cancelled
  4. The spawned task sends Error { code: "cancelled", ... } via tx
```

### 4.4 Session Lifecycle

- **`/new`**: Generates a UUIDv4 session ID, creates `SessionData { info, messages: vec![], cancel_token: new() }`, stores in `sessions` map. Returns `SessionInfo`.
- **`/switch <id>`**: Updates `State::active_session`. The `LocalBackend` is not involved — session switching only affects the TUI's view state.
- **`/clear`**: Clears `SessionState::messages` in the TUI state AND `SessionData::messages` in the `LocalBackend` (via `backend.clear_session(id)`). Both sides reset so the next prompt starts with a clean message history.
- **`/model <id>`**: Updates `LocalBackend.model`. Active session is not affected — a new session must be created with `/new` to use the new model. Displays a status notification: *"Model changed to X. Start a new session (/new) to use it."*

---

## 5. StreamingProvider

Defined in `crates/tui/src/backend/streaming_provider.rs`.

### 5.1 Purpose

Intercept the `AssistantMessageEventStream` produced by an inner `LlmProvider`, forward each event through an `mpsc::Sender<ServerEvent>` (after mapping), and yield the same events to the caller (so the agent loop continues to work). `StreamingProvider` is a pure pass-through — it does not track tool call state or execution results. Tool execution sequencing is handled by the spawned task in `LocalBackend::send_message()`.

### 5.2 Structure

```rust
pub struct StreamingProvider {
    /// The real LLM provider (Anthropic, OpenAI, etc.).
    inner: Arc<dyn LlmProvider>,
    /// Channel to forward mapped events to the TUI.
    tx: mpsc::Sender<ServerEvent>,
    /// Per-call_id → name mapping built during streaming.
    /// Populated when ToolCallEnd reveals the full ToolCall.
    tool_names: Mutex<HashMap<String, String>>,
}
```

### 5.3 `stream()` Implementation

```
StreamingProvider::stream(model, context, options, signal):
  1. Call inner.stream(model, context, options, signal)
  2. Get the inner stream
  3. Wrap with a custom Stream adapter that:
     For each event in inner stream:
       a. Map event to Option<ServerEvent> (see §6)
       b. If Some(event): try_send via tx (non-blocking; if full, warn! and skip)
       c. Yield the original event to the agent loop
  4. Return the wrapped stream
```

**Backpressure handling:** If the `mpsc` channel is full, the event is silently dropped with a `warn!` log. The TUI's 32-element buffer provides ample headroom for normal rendering (text deltas arrive every ~50ms, TUI renders every ~16ms).

### 5.4 Tool Execution Sequencing

The agent loop internally handles tool call execution after the LLM stream delivers `Done { reason: ToolUse }`. `StreamingProvider` does not participate in tool execution — it only forwards the LLM's intent to call tools.

After `AgentLoop::run()` returns `Vec<AgentMessage>`, the spawned task in `LocalBackend::send_message()` inspects the results:

```
After AgentLoop::run() returns Vec<AgentMessage>:
  For each message in results:
    If ToolResult { tool_call_id, content, is_error }:
      Emit ServerEvent::ToolCallDone {
          call_id: tool_call_id,
          result: content.first().map(|c| format_content(c)),
          is_error,
      }
  After all messages:
    Emit ServerEvent::TurnEnd {
        stop_reason: "stop",
        usage: last_assistant_usage,
    }
```

### 5.5 Tool Execution Flow (Detailed)

```
LLM stream events (forwarded by StreamingProvider):
  ToolCallStart { content_index: 0 }
    → buffered (content_index 0 = pending tool call, no id/name yet)
  ToolCallDelta { content_index: 0, delta: "{\"path\":" }
    → buffered by content_index 0
  ToolCallEnd { content_index: 0, tool_call: { id: "call_a", name: "read_file", args: {...} } }
    → now we have call_id and name
    → emit ServerEvent::ToolCallStarted { call_id: "call_a", name: "read_file" }
    → emit buffered ToolCallDelta { call_id: "call_a", delta: "{\"path\":" }
    → store ("call_a" → "read_file") in tool_names
```

**Note:** `ToolCallStart` only carries `content_index` (no `tool_call`); `ToolCallDelta` only carries `content_index` + `delta` (no `call_id`). The full `ToolCall` (with `id` and `name`) is only available in `ToolCallEnd`. Therefore deltas must be buffered by `content_index` and replayed after `ToolCallEnd` reveals the tool identity.

```
After AgentLoop returns:
  ToolResult { tool_call_id: "call_a", content: "fn main() {}", is_error: false }
    → spawned task emits ServerEvent::ToolCallDone {
        call_id: "call_a",
        result: Some("fn main() {}"),
        is_error: false,
    }

(If stop_reason was ToolUse, next turn stream begins — already forwarded above)

Final turn:
  Done { reason: Stop, message: { usage: { input: 500, output: 200 } } }
    → StreamingProvider forwards (mapped to event, but TurnEnd is suppressed — see §6)
    → AgentLoop returns results
    → Spawned task emits single ServerEvent::TurnEnd {
        stop_reason: "stop",
        usage: Some(UsageInfo { input_tokens: 500, output_tokens: 200 }),
    }
```

---

## 6. Event Mapping

Defined in `crates/tui/src/backend/event_mapper.rs`.

### 6.1 Mapping Table

| `AssistantMessageEvent` | `ServerEvent` | Notes |
|---|---|---|
| `Start { partial }` | `MessageStart { message_index }` | Message index increments per turn |
| `TextDelta { content_index, delta }` | `TextDelta { delta }` | content_index reserved for future multi-block support |
| `ThinkingDelta { content_index, delta }` | `ThinkingDelta { content_index, delta }` | Direct pass-through |
| `ToolCallStart { content_index }` | *(buffered — no event emitted)* | Only `content_index` is known; `call_id` and `name` are not available until `ToolCallEnd` |
| `ToolCallDelta { content_index, delta }` | *(buffered — no event emitted)* | Buffered by `content_index`. Replayed after `ToolCallEnd` as `ToolCallDelta { call_id, delta }` |
| `ToolCallEnd { content_index, tool_call }` | `ToolCallStarted { call_id, name }` + replay buffered deltas | This is the first event with `call_id` and `name`. Emit `ToolCallStarted`, then emit buffered `ToolCallDelta` items |
| `Done { reason, message }` | *(suppressed for intermediate turns; see §6.4)* | `Done` fires for each turn including `ToolUse` turns. TurnEnd is emitted once by the spawned task after `AgentLoop::run()` completes |
| `Error { error }` | `Error { code, message }` | Map `error.stop_reason` + `error.error_message` |
| `TextStart`, `TextEnd`, `ThinkingStart`, `ThinkingEnd` | *(ignored)* | These are structural events; content is accumulated in deltas |

### 6.2 Tool Call Event Sequencing

Because `call_id` and `name` are not available until `ToolCallEnd`, the event mapper buffers deltas by `content_index`:

```
ToolCallStart { content_index: 0 }
  → mark content_index 0 as pending in a BTreeMap<usize, Vec<String>>
ToolCallDelta { content_index: 0, delta: "{\"path\":" }
  → push delta to buffer[0]
ToolCallDelta { content_index: 0, delta: "\"src/main.rs\"}" }
  → push delta to buffer[0]
ToolCallEnd { content_index: 0, tool_call: { id: "call_a", name: "read_file" } }
  → emit ToolCallStarted { call_id: "call_a", name: "read_file" }
  → for each buffered delta in buffer[0]:
      emit ToolCallDelta { call_id: "call_a", delta }
  → store "call_a" → "read_file" in StreamingProvider.tool_names
  → clear buffer[0]
```

### 6.3 Error Event Mapping

| LLM error condition | `ServerEvent` |
|---|---|
| Provider auth failure (401) | `Error { code: "auth_failed", message: "..." }` |
| Rate limit (429) | `Error { code: "rate_limited", message: "..." }` |
| Context overflow | `Error { code: "context_overflow", message: "..." }` |
| Streaming connection lost | `Error { code: "stream_error", message: "..." }` |
| Tool execution failure | `ToolCallDone { is_error: true, result: Some("error message") }` |
| User interrupt | `Error { code: "cancelled", message: "Interrupted" }` |

### 6.4 Implementation

```rust
use std::collections::BTreeMap;

/// Mutable state for the event mapper, held by StreamingProvider.
pub struct EventMapperState {
    /// Buffered tool call deltas, keyed by content_index (not call_id — not
    /// available until ToolCallEnd).  BTreeMap is used to guarantee insertion
    /// order (HashMap would randomise key iteration).
    pending_tool_deltas: BTreeMap<usize, Vec<String>>,
    /// Per-call_id → tool_name mapping, built when ToolCallEnd reveals the
    /// full ToolCall.  Used by the spawned task to provide names for
    /// ToolCallDone events.
    tool_names: HashMap<String, String>,
}

pub fn map_stream_event(
    event: &AssistantMessageEvent,
    state: &Mutex<EventMapperState>,
    turn_index: u64,
) -> Vec<ServerEvent> {
    match event {
        AssistantMessageEvent::Start { .. } => {
            vec![ServerEvent::MessageStart { message_index: turn_index }]
        }
        AssistantMessageEvent::TextDelta { delta, .. } => {
            vec![ServerEvent::TextDelta { delta: delta.clone() }]
        }
        AssistantMessageEvent::ThinkingDelta { content_index, delta } => {
            vec![ServerEvent::ThinkingDelta {
                content_index: *content_index,
                delta: delta.clone(),
            }]
        }
        // ToolCallStart only has content_index — no call_id or name yet.
        // Buffer deltas by content_index until ToolCallEnd arrives.
        AssistantMessageEvent::ToolCallStart { content_index, .. } => {
            let mut state = state.lock().expect("mutex poisoned");
            state.pending_tool_deltas.entry(*content_index).or_default();
            Vec::new() // no event emitted yet
        }
        AssistantMessageEvent::ToolCallDelta { content_index, delta } => {
            let mut state = state.lock().expect("mutex poisoned");
            state
                .pending_tool_deltas
                .entry(*content_index)
                .or_default()
                .push(delta.clone());
            Vec::new() // buffered
        }
        // ToolCallEnd reveals the full ToolCall with id and name.
        // Emit ToolCallStarted + replay any buffered deltas.
        AssistantMessageEvent::ToolCallEnd { content_index, tool_call } => {
            let mut state = state.lock().expect("mutex poisoned");
            state
                .tool_names
                .insert(tool_call.id.clone(), tool_call.name.clone());

            let mut events = Vec::new();
            events.push(ServerEvent::ToolCallStarted {
                call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
            });

            // Replay buffered deltas for this content_index
            if let Some(deltas) = state.pending_tool_deltas.remove(content_index) {
                for delta in deltas {
                    events.push(ServerEvent::ToolCallDelta {
                        call_id: tool_call.id.clone(),
                        delta,
                    });
                }
            }
            events
        }
        // Done fires for every turn (including intermediate ToolUse turns).
        // Do NOT emit TurnEnd here — the spawned task emits a single TurnEnd
        // after AgentLoop::run() completes.  ToolCallDone events are emitted
        // by the spawned task after inspecting AgentLoop results.
        AssistantMessageEvent::Done { .. } => {
            Vec::new() // suppressed
        }
        AssistantMessageEvent::Error { error } => {
            vec![ServerEvent::Error {
                code: format!("{:?}", error.stop_reason).to_lowercase(),
                message: error.error_message.clone().unwrap_or_default(),
            }]
        }
        _ => Vec::new(),
    }
}
```

**Design notes:**
- Returns `Vec<ServerEvent>` (not `Option`) because a single stream event can produce multiple TUI events (e.g., `ToolCallEnd` → `ToolCallStarted` + N × `ToolCallDelta`).
- `BTreeMap<usize, Vec<String>>` for delta buffering — `usize` key is `content_index` (deterministic ordering), `Vec<String>` accumulates partial JSON arguments.
- `Done` and `ToolCallEnd` in the LLM protocol are different: `ToolCallEnd` means "the LLM finished describing this tool call" (arguments complete); `Done` means "the LLM finished this turn" (stop reason may be `ToolUse` or `Stop`). Tool execution happens after `Done` with `ToolUse`, inside the agent loop.
- `tool_names` is read by the spawned task after `AgentLoop::run()` returns, so it can include the tool name in `ToolCallDone` events if desired.

---

## 7. Configuration

### 7.1 New CLI Args

Extend `CliArgs` in `config.rs`:

```rust
#[derive(Parser)]
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
    #[arg(long, env = "PANDARIA_PROVIDER", default_value = "anthropic")]
    pub provider: String,

    /// Model name for local mode
    #[arg(long, env = "PANDARIA_MODEL", default_value = "claude-sonnet-4-20250514")]
    pub model: String,

    /// Theme name
    #[arg(long)]
    pub theme: Option<String>,

    /// Config file path
    #[arg(long, env = "PANDARIA_CONFIG")]
    pub config: Option<String>,
}
```

### 7.2 Config Struct Changes

```rust
pub struct Config {
    pub server: Option<ServerConfig>,   // None → local mode
    pub auth: AuthConfig,
    pub llm: LlmConfig,                 // NEW: provider + model
    pub ui: UiConfig,
    pub keys: KeysConfig,
}

pub struct LlmConfig {
    pub provider: String,   // "anthropic", "openai", "google", "mistral"
    pub model: String,      // e.g., "claude-sonnet-4-20250514"
}

pub struct ServerConfig {
    pub url: String,
    pub timeout_secs: u64,
}
```

### 7.3 Mode Selection Logic

```
If --local flag is set:
  → local mode
Else if --url is set or PANDARIA_URL is set:
  → remote mode (HttpBackend, deferred)
Else:
  → local mode (default when no server URL)
```

### 7.4 Provider Resolution

The `--provider` / `PANDARIA_PROVIDER` value maps to an `LlmProvider` implementation:

| Value | Provider struct | Requires |
|---|---|---|
| `anthropic` | `AnthropicProvider` | `PANDARIA_TOKEN` as `x-api-key` |
| `openai` | `OpenAiProvider` | `PANDARIA_TOKEN` as `Bearer` |
| `google` | `GoogleProvider` | `PANDARIA_TOKEN` as API key |
| `mistral` | `MistralProvider` | `PANDARIA_TOKEN` as `Bearer` |

Provider is selected once at startup. The `/model` command can switch models within the same provider.

**Import note:** Only `MistralProvider` is publicly re-exported from `ai-provider` (`llm_client::MistralProvider`). Other providers require full module paths:
```rust
use llm_client::providers::anthropic::AnthropicProvider;
use llm_client::providers::openai::OpenAiProvider;
use llm_client::providers::google::GoogleProvider;
```

### 7.5 Config File

```toml
# ~/.config/pandaria/tui/config.toml

# Omit the entire [server] section (or omit `url`) for local mode.
# Uncomment `url` to switch to remote mode (future).
# [server]
# url = "http://localhost:8080"

[auth]
token = "${PANDARIA_TOKEN}"

[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"

[ui]
max_history = 500
show_tool_calls = true
syntax_theme = "base16-ocean.dark"
scrollback = 1000

[keys]
"app.quit" = ["ctrl+c", "ctrl+d"]
```

**Mode selection from config:** When `ServerConfig` is `None` (i.e., no `[server]` section or `url` is omitted), the TUI defaults to local mode. When `url` is present and `--local` is not set, remote mode is used (deferred).

---

## 8. File Changes

### 8.1 New Files

| File | Lines (est.) | Purpose |
|---|---|---|
| `crates/tui/src/backend.rs` | ~40 | `Backend` trait definition + `mod` declarations |
| `crates/tui/src/backend/local.rs` | ~250 | `LocalBackend`: session CRUD, send_message flow, interrupt |
| `crates/tui/src/backend/streaming_provider.rs` | ~100 | `StreamingProvider` wrapper + custom `Stream` adapter |
| `crates/tui/src/backend/event_mapper.rs` | ~120 | `map_stream_event()`, tool call state tracking, TurnEnd/Error generation |

### 8.2 Modified Files

| File | Change |
|---|---|
| `crates/tui/Cargo.toml` | Add `agent-core`, `ai-provider`, `secrecy` deps |
| `crates/tui/src/lib.rs` | Add `pub mod backend;` |
| `crates/tui/src/app.rs` | Replace `rest: RestClient` + `reqwest_client` with `backend: Box<dyn Backend>`. Update `App::new()`, `submit_input()`, `handle_overlay_confirm()` |
| `crates/tui/src/main.rs` | Provider construction, mode selection, `LocalBackend` init |
| `crates/tui/src/config.rs` | Add `LlmConfig`, make `ServerConfig` optional, new CLI args |

### 8.3 Unchanged Files

All UI widgets (`chat_view.rs`, `tool_call.rs`, `thinking.rs`, `status_bar.rs`, etc.), overlays, `state.rs`, `command.rs`, `markdown.rs`, `paste.rs`, `client/*` (preserved for future `HttpBackend`), and `ui/theme.rs` require **zero changes**.

---

## 9. Data Flow

### 9.1 User Message Submission (Happy Path)

```
User types "What does this code do?" and presses Enter
  │
  ▼
App::submit_input()
  ├── Renders User message block in ChatView
  ├── Transitions AppState → Busy
  ├── Calls backend.send_message(session_id, "What does this code do?")
  │     │
  │     ▼
  │   LocalBackend::send_message()
  │     ├── Creates mpsc::channel::<ServerEvent>(32)
  │     ├── Clones session_id, provider, hook_dispatcher, tools
  │     └── Spawns tokio task:
  │           │
  │           ▼
           │         Creates StreamingProvider(provider, tx)
           │         Runs AgentLoop::run(system_prompt, messages, cancel_token):
           │           │
  │           ▼
         │         AgentLoop::run():
           │           │
           │           ├─▶ LLM Stream via StreamingProvider:
           │           │     ├─ TextDelta("Let") → tx.send(TextDelta { delta: "Let" })
           │           │     ├─ TextDelta(" me") → tx.send(TextDelta { delta: " me" })
           │           │     ├─ TextDelta(" look...") → tx.send(TextDelta { delta: " look..." })
           │           │     ├─ ThinkingDelta("Hmm...") → tx.send(ThinkingDelta { ... })
           │           │     │
           │           │     ├─ ToolCallStart { content_index: 0 } → (buffered)
           │           │     ├─ ToolCallDelta { content_index: 0, delta: "{\"path\":\"src/main.rs\"}" }
           │           │     │     → (buffered by content_index 0)
           │           │     └─ ToolCallEnd { content_index: 0, tool_call: { id: "a", name: "read_file" } }
           │           │           → tx.send(ToolCallStarted { call_id: "a", name: "read_file" })
           │           │           → tx.send(ToolCallDelta { call_id: "a", delta: "{\"path\":\"src/main.rs\"}" })
           │           │
           │           ├─▶ Tool Execution (in agent loop):
           │           │     └─ Tool result: "fn main() {}"
           │           │
           │           ├─▶ Next turn (if stop_reason was ToolUse):
           │           │     └─ LLM sees tool result, streams more text:
           │           │       → tx.send(TextDelta { delta: "The file contains..." })
           │           │
           │           └─▶ AgentLoop returns Vec<AgentMessage>
           │                 ├─ Spawned task scans for ToolResult messages:
           │                 │   → tx.send(ToolCallDone { call_id: "a",
           │                 │        result: "fn main() {}", is_error: false })
           │                 └─ Emits single TurnEnd:
           │                     → tx.send(TurnEnd { stop_reason: "stop", usage: { ... } })
  │
  ├── Stores rx as self.server_rx
  │
  ▼
Event loop receives ServerEvent from rx:
  ├── MessageStart → (no-op)
  ├── TextDelta → update StreamingBuffer, re-render ChatView
  ├── ThinkingDelta → update ThinkingBlock
  ├── ToolCallStarted → create ToolCallWidget with pending border
  ├── ToolCallDelta → accumulate arguments
  ├── ToolCallDone → finalize result, success/error border
  ├── TurnEnd → finalize assistant message, AppState → Connected
  └── Error → show error banner, AppState → Connected
```

### 9.2 Interrupt Flow

```
User presses Esc during streaming
  │
  ▼
App::handle_key_event(Esc) while Busy
  ├── backend.interrupt(session_id)
  │     └─ LocalBackend: session_data.cancel_token.cancel() → CancellationToken cancelled
  ├── Marks current assistant message as Aborted
  ├── AppState → Connected
  │
  ▼
CancellationToken propagates:
  ├── LLM stream: returns LlmError::Cancelled
  ├── Agent loop: catches Cancelled, stops iteration
  └── StreamingProvider: sends Error { code: "cancelled", ... }
```

### 9.3 Slash Command Flow

```
User types "/new my session" and presses Enter
  │
  ▼
App::submit_input()
  ├── Detects '/' prefix
  ├── Calls handle_overlay_confirm("/new my session")
  │     └── Command::parse() → Command::NewSession { title: Some("my session") }
  │
  ▼
Command::NewSession dispatch:
  ├── backend.create_session(Some("my session"), &current_model)
  │     └── LocalBackend:
  │           ├── Generate session_id (UUIDv4)
  │           ├── Create SessionData { info, messages: vec![], cancel_token: new() }
  │           ├── Store in sessions map
  │           └── Return SessionInfo { id, title: "my session", model, ... }
  │
  ├── Create new SessionState with the returned info
  ├── Insert into State::sessions
  └── Switch active_session to the new ID
```

---

## 10. Error Handling

| Scenario | User-Visible | Implementation |
|---|---|---|
| Missing API key in local mode | Config load fails with clear error message | `Config::load()` validates: `--local` or no `--url` → requires `--token` |
| Invalid API key (401 from LLM) | `ServerEvent::Error { code: "auth_failed" }` | `StreamingProvider` catches `LlmError::Auth`, maps to `Error` event |
| Rate limited (429) | `ServerEvent::Error { code: "rate_limited" }` | `LlmProvider` retries internally (3× exponential backoff). If all fail, `Error` event |
| Context overflow | `ServerEvent::Error { code: "context_overflow" }` | `LlmProvider` detects overflow, `LlmError::ContextOverflow` → `Error` event. User should `/clear` or start `/new` |
| Tool execution panic | `ToolCallDone { is_error: true }` | `AgentLoop` catches panics via `AssertUnwindSafe`. Error message included in result |
| Stream channel full (backpressure) | Event silently dropped with `warn!` log | 32-element buffer; dropping is safe (next delta provides updated accumulated content) |
| Session data not found for session ID | `send_message` returns `Err("session not found")` | Should not happen in normal use (session created before message sent). Treated as `Error` event |
| Double-submit (user sends message while busy) | Input ignored | `AppState::Busy` check in `submit_input()` prevents this |

### 10.1 API Key Security

- The API key is stored in `secrecy::SecretString` within `LlmProvider::StreamOptions`
- It is never logged, displayed, or included in error messages
- `Debug` output for `StreamOptions` displays `[REDACTED]` for the key field
- Config file parsing uses `${PANDARIA_TOKEN}` interpolation, deferring secret storage to the environment

---

## 11. Dependency Additions

Add to `crates/tui/Cargo.toml`:

```toml
[dependencies]
# ... existing deps ...

# Internal workspace crates
agent-core = { path = "../agent-core" }
ai-provider = { path = "../ai-provider" }

# Secret handling for API keys
secrecy = "0.8"
```

No new external dependencies beyond `secrecy` (already used by `ai-provider`).

---

## 12. Testing Strategy

### 12.1 Unit Tests

| Test | What it verifies |
|---|---|
| `event_mapper::test_text_delta_mapping` | `TextDelta` event is mapped correctly |
| `event_mapper::test_tool_call_end_mapping` | `ToolCallEnd` → `ToolCallStarted` + buffered `ToolCallDelta` events |
| `event_mapper::test_tool_call_delta_buffering` | Deltas are buffered by content_index and replayed after ToolCallEnd |
| `event_mapper::test_done_is_suppressed` | `Done` event does not produce TurnEnd |
| `event_mapper::test_error_mapping` | `Error` → `ServerEvent::Error` |
| `streaming_provider::test_events_forwarded` | Events pass through to mpsc receiver |
| `streaming_provider::test_backpressure_no_panic` | Channel full → event dropped, no panic |
| `local::test_create_session` | `create_session()` returns valid `SessionInfo` |
| `local::test_send_message_with_echo_provider` | Full flow with `EchoProvider` → correct ServerEvent sequence |
| `local::test_interrupt` | `interrupt()` sets `CancellationToken`, stream returns `Cancelled` |

### 12.2 Integration Tests

| Test | What it verifies |
|---|---|
| `test_local_mode_full_flow` | Create session → send message → receive streaming events → turn end |
| `test_multi_turn_tool_use` | Message triggers tool call → tool executed → result fed back → final response |
| `test_interrupt_while_streaming` | Send message → interrupt mid-stream → Aborted status |
| `test_multiple_sessions` | Create 2 sessions, send messages to each, verify isolation |

Integration tests use the `EchoProvider` and mock tool providers from `agent-core`'s test suite. No external network required.

### 12.3 Manual Verification

| Test scenario | How to verify |
|---|---|
| Real LLM call (Anthropic) | Set `PANDARIA_TOKEN`, run `pandaria-tui`, type message, see streaming response |
| Real LLM call (OpenAI) | Same with `--provider openai --model gpt-4o` |
| Tool use streaming | Use a session with file-reading tools, ask "read src/main.rs" |
| Interrupt with Esc | Send a long-answer query, press Esc mid-response |
| `/new` command | Type `/new test`, verify new session appears in tabs |
| `/model` command | Type `/model`, select a different model from overlay |

---

## 13. Key Design Decisions

| Decision | Rationale |
|---|---|
| **`Backend` trait with `mpsc::Receiver` return** | The TUI's existing `server_rx` field holds exactly this type. No changes needed to `handle_server_event()` or the event loop. Enables swapping local/remote backends without App changes. |
| **`StreamingProvider` wraps `LlmProvider`, not `AgentLoop`** | The `LlmProvider` trait is the natural interception point — it's where the stream originates. Wrapping at the provider level requires zero changes to `agent-core`. The agent loop continues to work identically. |
| **Reusing `ServerEvent` for local mode** | The TUI already has complete rendering logic for `ServerEvent` variants. Duplicating rendering for a "local event" type would create divergence. The `ServerEvent` enum is a protocol that both backends speak. |
| **`Arc<tokio::sync::Mutex<HashMap<...>>>` + remove/re-insert pattern** | `AgentLoop::run()` is an async operation. Using `tokio::sync::Mutex` and removing the session before the prompt, then re-inserting with updated history afterwards, ensures the lock is never held across an `.await` point. No deadlock risk. |
| **Batch tool execution, not streaming** | `AgentLoop` internally executes tools after the LLM stream completes. Intercepting tool execution mid-loop would require modifying `agent-core`. Instead, the spawned task inspects `AgentLoop::run()` results and emits `ToolCallDone` events for any `ToolResult` messages found. This is simple and correct. |
| **`Done` suppressed for intermediate turns** | The LLM stream emits `Done` for every turn — including intermediate `ToolUse` turns where more turns follow. The event mapper suppresses these. A single `TurnEnd` is emitted by the spawned task after `AgentLoop::run()` returns, regardless of how many turns occurred. |
| **API key = `--token` / `PANDARIA_TOKEN`** | Avoids introducing a separate `--api-key` flag. In local mode, the token is the LLM API key. In remote mode (future), it's the server auth token. One concept, one CLI arg. |
| **Provider resolution from `--provider` string** | String-based provider selection with a static registry (`match provider { "anthropic" => ..., "openai" => ..., ... }`). No dynamic discovery needed for an MVP with 4 providers. |
| **`--local` flag as override** | When `--url` is set for remote mode, `--local` overrides to local. When neither is set, defaults to local. Clear precedence rules. |
| **No HTTP in local mode** | `LocalBackend` never constructs HTTP requests to a pandaria server. LLM API calls go through `LlmProvider::stream()` which uses reqwest internally, but this is the provider's implementation detail, not the backend's. |
| **Noop `HookDispatcher` initially** | Extension hooks add complexity without immediate value for local mode. The `HookDispatcher` trait's default no-op implementations are sufficient. Extension support can be added later through `LocalBackend` configuration. |

---

## 14. Out of Scope

- **`HttpBackend` implementation** — The existing `RestClient` + SSE code is preserved but not wired as a `Backend` impl. This is trivial follow-up work.
- **Session persistence** — No `SessionStore` configured. Sessions are lost on TUI exit.
- **Extension hooks beyond noop** — Builtins (audit, rate-limit, tool-guard) are not wired.
- **Multi-session concurrent prompts** — The TUI processes one message at a time. Sending a message to session A while session B is streaming is not supported.
- **Model auto-discovery** — Model list is hardcoded or provided via `--model`. The `ModelRegistry` from `ai-provider` can be integrated later.
- **Server mode** — Building the `api-gateway` crate and `src/main.rs` server binary is a separate effort.
- **Provider hot-swap** — Changing provider requires restarting the TUI.
- **Per-session model selection** — All sessions under a `LocalBackend` use the same model. Per-session models require storing model preference per `SessionData`.
- **Streaming tool output** — Tool results are displayed only after execution completes, not incrementally during execution.
- **Token cost tracking** — Usage stats are displayed but not aggregated across turns or sessions.
- **OAuth provider authentication** — Only API key authentication is supported.
- **AWS Bedrock provider** — Requires the `bedrock` feature flag and additional AWS credential configuration.
