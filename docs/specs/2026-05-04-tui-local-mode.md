# Pandaria TUI — Local Mode Integration Spec

**Date:** 2026-05-04
**Status:** Draft
**Reference:** `docs/specs/2026-05-02-tui-design.md`, `docs/specs/2026-05-02-agent-core.md`, `docs/specs/2026-05-02-llm-client.md`

---

## 1. Purpose

Integrate `agent-core` and `llm-client` directly into the TUI crate so that the TUI works as a **self-contained standalone CLI tool** without requiring a running pandaria server.

The TUI currently communicates with a server via SSE + HTTP REST, but the server binary does not exist yet. This spec defines a **local mode** that embeds the agent runtime in-process, while preserving the existing HTTP/SSE client path for future server mode via a `Backend` trait abstraction.

**Goals:**
- Make all TUI slash commands (`/new`, `/switch`, `/model`, `/clear`, etc.) functional
- Stream LLM responses to the TUI in real-time (text, thinking, tool calls)
- Zero changes to `agent-core` and `llm-client` crates — integration happens entirely in the TUI crate
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
     │ SessionActor│        └─────────────┘
     │ Streaming   │
     │ Provider    │
     └──────┬──────┘
            │
     ┌──────▼──────────────────────┐
     │ agent-core::SessionActor    │
     │   ├── prompt(text)          │
     │   ├── abort()               │
     │   └── messages()            │
     │                             │
     │ agent-core::AgentLoop       │
     │   └── run() → stream        │
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

    /// Send a user message to the session.
    ///
    /// Returns a `Receiver<ServerEvent>` that yields streaming events
    /// (text deltas, tool calls, turn end, errors). The caller should poll
    /// this receiver until `TurnEnd` or `Error` is received, then drop it.
    ///
    /// This is the same channel type used by the SSE client path —
    /// `App::server_rx` can hold either without code changes.
    async fn send_message(
        &self,
        session_id: &str,
        content: &str,
    ) -> Result<mpsc::Receiver<ServerEvent>, String>;

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
    /// Active sessions, keyed by session ID.
    sessions: Arc<Mutex<HashMap<String, SessionActor>>>,

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
```

**Design decisions:**

- **`Arc<Mutex<HashMap<...>>>`** — The TUI runs on a single thread (the tokio event loop). The `Mutex` guards access to `SessionActor` (which requires `&mut self` for `prompt()`), and `Arc` allows sharing between the event loop and spawned streaming tasks. No actual contention occurs because the TUI serializes all user actions.
- **Shared provider** — All sessions use the same `LlmProvider` instance. This avoids re-creating HTTP connection pools per session.
- **Noop `HookDispatcher`** — Local mode uses `AllowAllDispatcher` (all hooks default to no-op). Extension hooks can be wired in later without changing the `Backend` trait.
- **`model: Mutex<String>`** — The current model name is shared across all sessions under `LocalBackend`. Per-session model selection is deferred (single-model usage is sufficient for local mode MVP).

### 4.2 `send_message` Implementation

```
LocalBackend::send_message(session_id, content):
  1. Lock sessions, get SessionActor
  2. Create mpsc::channel::<ServerEvent>(32)
  3. Clone Arc<sessions>, Arc<provider>, tx
  4. Spawn tokio task:
     a. Wrap provider with StreamingProvider(provider, tx)
     b. Build new SessionActor with StreamingProvider
     c. Call session.prompt(content)
     d. Map any remaining messages to ServerEvent (catch final tool results)
     e. Send TurnEnd or Error
     f. Drop tx (signals end of stream to receiver)
  5. Return rx to caller
```

**Important:** `SessionActor::prompt()` takes `&mut self`. The spawned task owns the `SessionActor` (via `Arc<Mutex<>>` lock held for the duration of the prompt). No other operation on this session can proceed until the prompt completes — this matches the TUI's single-turn-at-a-time model.

### 4.3 `interrupt` Implementation

```
LocalBackend::interrupt(session_id):
  1. Lock sessions, get SessionActor
  2. Call session.abort()
  3. The CancellationToken propagates to the LLM stream, which returns LlmError::Cancelled
  4. The spawned task sends Error { code: "cancelled", ... } via tx
```

### 4.4 Session Lifecycle

- **`/new`**: Generates a UUIDv4 session ID, creates `SessionActor` with the configured model and provider, stores in `sessions` map.
- **`/switch <id>`**: Updates `State::active_session`. The `LocalBackend` is not involved — session switching only affects the TUI's view state.
- **`/clear`**: Clears `SessionState::messages` in the TUI state. Does not touch `SessionActor` (agent history remains but UI view is cleared).
- **`/model <id>`**: Updates `LocalBackend.model`. Future sessions use the new model. Active session is not affected (a new session must be created with `/new`).

---

## 5. StreamingProvider

Defined in `crates/tui/src/backend/streaming_provider.rs`.

### 5.1 Purpose

Intercept the `AssistantMessageEventStream` produced by an inner `LlmProvider`, forward each event through an `mpsc::Sender<ServerEvent>` (after mapping), and yield the same events to the caller (so the agent loop continues to work).

### 5.2 Structure

```rust
pub struct StreamingProvider {
    inner: Arc<dyn LlmProvider>,
    tx: mpsc::Sender<ServerEvent>,
    /// Tracks pending tool calls by call_id for event mapping.
    pending_tool_calls: Mutex<HashMap<String, ToolCall>>,
}
```

### 5.3 `stream()` Implementation

```
StreamingProvider::stream(model, context, options, signal):
  1. Call inner.stream(model, context, options, signal)
  2. Get the inner stream
  3. Wrap with a custom Stream adapter that:
     For each event in inner stream:
       a. Map event to ServerEvent (see Section 6)
       b. Send ServerEvent via tx (non-blocking; if full, log warning and skip)
       c. Yield the original event to the agent loop
  4. Return the wrapped stream
```

**Backpressure handling:** If the `mpsc` channel is full, the event is silently dropped with a `warn!` log. The TUI's 32-element buffer provides ample headroom for normal rendering (text deltas arrive every ~50ms, TUI renders every ~16ms).

### 5.4 Tool Execution Interception

The agent loop receives tool calls from the LLM stream and executes them via `ToolExecutor`. `StreamingProvider` must:

1. Forward `ToolCallStart`/`ToolCallDelta` events to the TUI (so the user sees the tool being called)
2. Track which tool calls are pending
3. After the stream delivers `Done { reason: ToolUse }`, the agent loop executes tools. `StreamingProvider` cannot intercept this directly.

Instead, the `LocalBackend::send_message` spawned task handles tool execution sequencing:

```
After SessionActor::prompt() returns Vec<AgentMessage>:
  For each AgentMessage in results:
    If AssistantMessage with tool calls:
      Already forwarded ToolCallStarted/Delta during stream
      Now need to find matching ToolResult messages
    If ToolResult:
      Emit ToolCallDone { call_id, result, is_error }
  After all messages:
    Emit TurnEnd { stop_reason, usage }
```

**Alternative approach (considered but rejected):** Modifying `AgentLoop` to accept a streaming callback. This would require changes to `agent-core` and couples the loop to TUI-specific event types. The batch approach keeps the integration surface minimal.

### 5.5 Tool Execution Flow (Detailed)

```
LLM stream events (from StreamingProvider):
  ToolCallStart { call_id: "a", name: "read_file" }
    → emit ServerEvent::ToolCallStarted { call_id: "a", name: "read_file" }
  ToolCallDelta { call_id: "a", delta: "{\"path\":" }
    → emit ServerEvent::ToolCallDelta { call_id: "a", delta: "{\"path\":" }
  ToolCallEnd { call_id: "a", tool_call: { id: "a", args: {"path": "src/main.rs"} } }
    → (tool call is now complete in LLM's view; agent loop will execute it)

Agent loop executes tool → ToolResult { tool_call_id: "a", content: "fn main() {}", is_error: false }
  → spawn task emits ServerEvent::ToolCallDone {
        call_id: "a",
        result: Some("fn main() {}"),
        is_error: false,
    }

(If stop_reason is ToolUse, next turn begins with tool results fed back...)

Eventually:
  Done { reason: Stop, message: { usage: { input: 500, output: 200 } } }
    → emit ServerEvent::TurnEnd {
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
| `ToolCallStart { content_index, tool_call }` | `ToolCallStarted { call_id, name }` | Extract `tool_call.id` and `tool_call.name` |
| `ToolCallDelta { content_index, delta }` | `ToolCallDelta { call_id, delta }` | call_id from current tool call context |
| `ToolCallEnd { content_index, tool_call }` | *(buffered — no event emitted)* | Tool execution happens after stream; result sent as `ToolCallDone` |
| `Done { reason, message }` | `TurnEnd { stop_reason, usage }` | With usage stats from `message.usage` |
| `Error { error }` | `Error { code, message }` | Map `error.stop_reason` + `error.error_message` |
| `TextStart`, `TextEnd`, `ThinkingStart`, `ThinkingEnd` | *(ignored)* | These are structural events; content is accumulated in deltas |

### 6.2 Tool Call State Machine

```
                    ┌─────────────────┐
                    │ ToolCallStarted │  ← LLM decides to call a tool
                    │ (pending border) │
                    └───────┬─────────┘
                            │ ToolCallDelta (streaming args)
                    ┌───────▼─────────┐
                    │ ToolCallDone    │  ← tool execution result
                    │ (success/error) │
                    └─────────────────┘
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
pub fn map_stream_event(
    event: &AssistantMessageEvent,
    pending_tool_calls: &mut HashMap<String, String>, // call_id → name
    turn_index: u64,
) -> Option<ServerEvent> {
    match event {
        AssistantMessageEvent::Start { .. } => {
            Some(ServerEvent::MessageStart { message_index: turn_index })
        }
        AssistantMessageEvent::TextDelta { delta, .. } => {
            Some(ServerEvent::TextDelta { delta: delta.clone() })
        }
        AssistantMessageEvent::ThinkingDelta { content_index, delta } => {
            Some(ServerEvent::ThinkingDelta { content_index: *content_index, delta: delta.clone() })
        }
        AssistantMessageEvent::ToolCallStart { content_index: _, tool_call } => {
            pending_tool_calls.insert(tool_call.id.clone(), tool_call.name.clone());
            Some(ServerEvent::ToolCallStarted {
                call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
            })
        }
        AssistantMessageEvent::ToolCallDelta { content_index: _, delta } => {
            // Delta applies to the most recently started tool call
            let call_id = pending_tool_calls.keys().last()?.clone();
            Some(ServerEvent::ToolCallDelta { call_id, delta: delta.clone() })
        }
        AssistantMessageEvent::ToolCallEnd { .. } => {
            // Tool call is complete in LLM's view; execution happens next
            // No event emitted here — ToolCallDone comes after tool execution
            None
        }
        AssistantMessageEvent::Done { reason, message } => {
            Some(ServerEvent::TurnEnd {
                stop_reason: format!("{:?}", reason).to_lowercase(),
                usage: Some(UsageInfo {
                    input_tokens: message.usage.input_tokens,
                    output_tokens: message.usage.output_tokens,
                }),
            })
        }
        AssistantMessageEvent::Error { error } => {
            Some(ServerEvent::Error {
                code: format!("{:?}", error.stop_reason).to_lowercase(),
                message: error.error_message.clone().unwrap_or_default(),
            })
        }
        _ => None,
    }
}
```

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

### 7.5 Config File

```toml
# ~/.config/pandaria/tui/config.toml
[server]
# url = "http://localhost:8080"     # Uncomment for remote mode

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
| `crates/tui/Cargo.toml` | Add `agent-core`, `llm-client`, `secrecy` deps |
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
  │         Runs SessionActor::prompt(text):
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
  │           │     ├─ ToolCallStart("read_file") → tx.send(ToolCallStarted { ... })
  │           │     ├─ ToolCallDelta("{\"path\":\"src/main.rs\"}") → tx.send(ToolCallDelta { ... })
  │           │     └─ ToolCallEnd → (buffered)
  │           │
  │           ├─▶ Tool Execution:
  │           │     └─ Tool result: "fn main() {}"
  │           │       → tx.send(ToolCallDone { call_id, result: "fn main() {}", is_error: false })
  │           │
  │           ├─▶ Next turn (if ToolUse):
  │           │     └─ LLM sees tool result, produces more text:
  │           │       → tx.send(TextDelta { delta: "The file contains..." })
  │           │
  │           └─▶ Done { reason: Stop }:
  │                 └─ tx.send(TurnEnd { stop_reason: "stop", usage: { ... } })
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
  │     └─ LocalBackend: session.abort() → CancellationToken cancelled
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
  │           ├── Create SessionActor(provider, hooks, system_prompt, tools)
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
| `SessionActor` not found for session ID | `send_message` returns `Err("session not found")` | Should not happen in normal use (session created before message sent). Treated as `Error` event |
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
llm-client = { path = "../llm-client" }

# Secret handling for API keys
secrecy = "0.8"
```

No new external dependencies beyond `secrecy` (already used by `llm-client`).

---

## 12. Testing Strategy

### 12.1 Unit Tests

| Test | What it verifies |
|---|---|
| `event_mapper::test_text_delta_mapping` | `TextDelta` event is mapped correctly |
| `event_mapper::test_tool_call_started_mapping` | `ToolCallStart` → `ToolCallStarted` with correct id/name |
| `event_mapper::test_turn_end_mapping` | `Done` → `TurnEnd` with usage stats |
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
| **`Arc<Mutex<HashMap<...>>>` for session storage** | `SessionActor::prompt()` requires `&mut self`. The TUI event loop serializes user actions, so contention is impossible. `Arc<Mutex<>>` is the simplest safe abstraction for the single-threaded TUI. |
| **Batch tool execution, not streaming** | `AgentLoop` internally executes tools after the LLM stream completes. Intercepting tool execution mid-loop would require modifying `agent-core`. Instead, the spawned task inspects `SessionActor::prompt()` results and emits `ToolCallDone` events for any `ToolResult` messages found. This is simple and correct. |
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
- **Model auto-discovery** — Model list is hardcoded or provided via `--model`. The `ModelRegistry` from `llm-client` can be integrated later.
- **Server mode** — Building the `api-gateway` crate and `src/main.rs` server binary is a separate effort.
- **Provider hot-swap** — Changing provider requires restarting the TUI.
- **Per-session model selection** — All sessions under a `LocalBackend` use the same model. Per-session models require storing model preference per `SessionActor`.
- **Streaming tool output** — Tool results are displayed only after execution completes, not incrementally during execution.
- **Token cost tracking** — Usage stats are displayed but not aggregated across turns or sessions.
- **OAuth provider authentication** — Only API key authentication is supported.
- **AWS Bedrock provider** — Requires the `bedrock` feature flag and additional AWS credential configuration.
