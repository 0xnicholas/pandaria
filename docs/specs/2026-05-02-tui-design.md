# Pandaria TUI Module — Specification

**Date:** 2026-05-02
**Status:** Draft
**Reference:** pi.dev TUI (`packages/tui/`, `packages/coding-agent/src/modes/interactive/`)

---

## 1. Purpose

`crates/tui/` is an **independent binary crate** that provides a terminal-based chat client connecting to the pandaria API Gateway via SSE + HTTP. It is the primary end-user interface for interacting with agents hosted on the pandaria server.

**Not a library.** The TUI crate produces a standalone binary (`pandaria-tui`) with no public API.

---

## 2. Transport

SSE (Server-Sent Events) for streaming responses + HTTP POST for sending messages and session operations.

```
POST   /api/v1/sessions/{id}/messages  →  send user message (creates new turn)
DELETE /api/v1/sessions/{id}/messages/current →  interrupt in-flight turn
GET    /api/v1/sessions/{id}/events    →  SSE stream (text delta, tool call, tool result, turn end, error)
POST   /api/v1/sessions                →  create session
GET    /api/v1/sessions                →  list sessions
GET    /api/v1/sessions/{id}           →  get session metadata
```

Authentication via `Authorization: Bearer <token>` header. Config priority (highest to lowest): CLI flags (`--token`, `--url`) → environment vars (`PANDARIA_TOKEN`, `PANDARIA_URL`) → config file (`~/.config/pandaria/tui/config.toml`) → built-in defaults.

---

## 3. Crate Structure

```
crates/tui/
  Cargo.toml
  README.md
  src/
    main.rs                  # tokio runtime entry, CLI arg parsing, app bootstrap
    app.rs                   # App state machine, event loop orchestrator
    state.rs                 # Global mutable state: messages per session, streaming buffer, connection status
    client/
      mod.rs
      auth.rs                # Token loading (config/env/cli), auth header injection
      sse.rs                 # SSE stream parser → mpsc::Sender<ServerEvent>
      rest.rs                # HTTP client (reqwest), request builder, error wrapper
      model.rs               # API types: ServerEvent, SessionInfo, ApiError
    ui/
      mod.rs
      layout.rs              # Root layout: ChatArea / InputArea split, overlay compositing
      theme.rs               # Semantic color palette, ANSI style presets
    widgets/
      mod.rs
      header.rs              # Session name, connection indicator, token usage
      chat_view.rs           # Scrollable message list with streaming append
      input_bar.rs           # Multi-line input, history, autocomplete stub
      tool_call.rs           # Expandable/collapsible tool call display
      status_bar.rs          # Connection status, model name, token budget
      session_tabs.rs        # Session switcher bar
    overlays/
      mod.rs                 # OverlayStack: push/pop/clear, focus routing, compositing
      command_palette.rs     # Command selector (/ prefix dispatch)
      session_list.rs        # Session browser overlay
      help.rs                # Keybinding reference
    command.rs               # Slash-command parser and dispatcher
    markdown.rs              # pulldown-cmark → ratatui styled Text, syntect code highlight
    paste.rs                 # Bracketed paste handler, large-paste marker storage
    config.rs                # Config struct, file/env/cli merge, validation
```

---

## 4. Dependencies

```toml
[dependencies]
ratatui = "0.29"
crossterm = { version = "0.28", features = ["event-stream"] }
tokio = { workspace = true, features = ["full"] }
reqwest = { version = "0.12", features = ["stream", "json"] }
eventsource-stream = "0.6"
serde = { workspace = true }
serde_json = { workspace = true }
pulldown-cmark = "0.12"
syntect = { version = "5", default-features = false, features = ["default-syntaxes", "default-themes"] }
clap = { version = "4", features = ["derive", "env"] }
directories = "5"
toml = "0.8"
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
wiremock = "0.6"
```

---

## 5. Screen Layout

```
 ┌─────────────────────────────────────────────────────┐
 │  pandaria · session: a1b2 · model: gpt-4o           │  HeaderBar
 ├─────────────────────────────────────────────────────┤
 │ [a1b2] [c3d4] [e5f6]                            [+] │  SessionTabs
 ├─────────────────────────────────────────────────────┤
 │                                                     │
 │  ┌ User ────────────────────────────────────────┐   │
 │  │  What does this code do?                     │   │
 │  └──────────────────────────────────────────────┘   │
 │  ┌ Assistant ───────────────────────────────────┐   │  ChatView
 │  │  Let me look at the file...                  │   │  (fills
 │  └──────────────────────────────────────────────┘   │   remaining
 │  ┌ Tool: read_file  collapse ▼ ─────────────────┐   │   vertical
 │  │  input:  {"path": "src/main.rs"}              │   │   space)
 │  │  output: fn main() {}                        │   │
 │  └──────────────────────────────────────────────┘   │
 │                                                     │
 ├─────────────────────────────────────────────────────┤
 │  Connected · tokens: 1.2k/4k (30%)                  │  StatusBar
 ├─────────────────────────────────────────────────────┤
 │  > Write a message or /command...                    │  InputBar
 └─────────────────────────────────────────────────────┘
```

### Overlay example (composited on top):

```
 ┌─────────────────────────────────────────────────────┐
 │  pandaria · session: a1b2 · model: gpt-4o           │
 ├─────────────────────────────────────────────────────┤
 │  ╔══ Select Model ═══════════════════════════╗       │
 │  ║  claude-sonnet-4                          ║       │
 │  ║  ▶ gpt-4o                                 ║       │
 │  ║  claude-opus-4                            ║       │
 │  ║                                           ║       │
 │  ║  [↑↓ navigate] [enter confirm] [esc back] ║       │
 │  ╚═══════════════════════════════════════════╝       │
 │  ┌ Assistant ───────────────────────────────────┐   │
 │  │  Let me look at the file...                  │   │
 │  └──────────────────────────────────────────────┘   │
 ├─────────────────────────────────────────────────────┤
 │  Connected · tokens: 1.2k/4k (30%)                  │
 ├─────────────────────────────────────────────────────┤
 │  >                                                   │
 └─────────────────────────────────────────────────────┘
```

---

## 6. Component Responsibilities

### 6.1 `App` (src/app.rs)

State machine with three states:

```
Disconnected ──▶ Connected ──▶ Busy (streaming)
     ▲              │              │
     └──────────────┴──────────────┘
```

- **Disconnected**: Shows connection config prompt. No SSE stream.
- **Connected**: Idle, waiting for user input. All UI interactive.
- **Busy**: SSE stream active, input bar shows cancel hint, Escape sends interrupt.

Owns: `AppState`, `Config`, `SessionStore`, `reqwest::Client`, SSE `JoinHandle`.

Event loop: `tokio::select!` on `crossterm::event::EventStream` + `mpsc::Receiver<ServerEvent>`.

### 6.2 `State` (src/state.rs)

```rust
struct State {
    sessions: HashMap<SessionId, SessionState>,
    active_session: SessionId,
    connection_status: ConnectionStatus,
}

struct SessionState {
    info: SessionInfo,
    messages: Vec<RenderedMessage>,
    streaming: Option<StreamingBuffer>,
    tool_calls: Vec<ToolCallWidget>,
}

/// A message ready to be displayed in the ChatView.
struct RenderedMessage {
    role: MessageRole,            // User, Assistant
    blocks: Vec<MessageBlock>,    // ordered content blocks (text, tool call, markdown)
    timestamp: SystemTime,
    is_complete: bool,            // false while streaming
}

enum MessageBlock {
    Text(Vec<Line<'static>>),     // rendered ratatui lines
    ToolCall(ToolCallWidget),
}

/// Accumulates streaming text deltas for the in-flight AssistantMessage.
struct StreamingBuffer {
    text_content: String,          // accumulated text so far
    pending_tool_calls: Vec<ToolCallWidget>,  // tool calls seen in this turn
}
```

### 6.3 Widgets

| Widget | Ratatui Primitive | Key Behavior |
|---|---|---|
| `HeaderBar` | `Paragraph` | Session name, model, connection icon |
| `SessionTabs` | `Tabs` | Active highlight, close button (X), new-session (+) |
| `ChatView` | `Paragraph` in scrollable region | Append-only, auto-scroll to bottom unless user scrolled up |
| `ToolCallWidget` | `Paragraph` + `Block` borders | Expand/collapse toggle, pending/success/error border color |
| `StatusBar` | `Paragraph` | Connection status, token usage bar, context usage % |
| `InputBar` | `Paragraph` (editable buffer) | History (↑↓), Tab autocomplete, Enter submit, Escape cancel |

### 6.4 Overlays

| Overlay | Trigger | Behavior |
|---|---|---|
| `CommandPalette` | `/` in input | Fuzzy-filter command list, Enter dispatches |
| `SessionList` | `/list` or `Ctrl+S` | Selectable session list, Enter switches, Delete removes |
| `Help` | `/help` or `F1` | Scrollable keybinding reference, Escape dismisses |
| `ModelSelector` | `/model` or `Ctrl+L` | Selectable model list, Enter confirms |

Overlay compositing: overlay lines are spliced into the base frame buffer at calculated row/col. Non-overlay content behind the overlay is dimmed. Only top-most overlay receives input.

---

## 7. Event Flow

```
┌──────────────┐    ┌────────────┐    ┌───────────┐    ┌─────────────┐
│ crossterm     │    │ App Event  │    │ State     │    │ ratatui     │
│ EventStream   │───▶│ Loop       │───▶│ Mutations │───▶│ Frame.draw()│
└──────────────┘    └────────────┘    └───────────┘    └─────────────┘
                           ▲
┌──────────────┐          │
│ SSE Stream   │──────────┘
│ (reqwest)    │  ServerEvent via mpsc
└──────────────┘
```

### 7.1 Keyboard Events

```
Key(Enter)        → if overlay focused: confirm overlay action
                    else: submit input text as UserMessage, send HTTP POST
Key(Esc)          → if overlay open: dismiss overlay
                    else if streaming: send interrupt HTTP DELETE
                    else: clear input
Key(Up/Down)      → if overlay focused: navigate overlay list
                    else if input focused: scroll input history
                    else: scroll chat view
Key(Tab)          → cycle focus: input → chat (scroll mode) → session tabs → input
Key(Shift+Tab)    → reverse cycle focus
Key(PageUp/Dn)    → scroll chat history one page
Key(Ctrl+C)       → if streaming: send interrupt
                    else: quit
Key(Ctrl+D)       → quit (only when input is empty)
Key(Ctrl+O)       → toggle all tool calls expanded
Key(Ctrl+L)       → open model selector overlay
Key(Ctrl+S)       → open session list overlay
Char('/')         → if input empty: open command palette
Char(ch)          → append to input buffer
```

### 7.2 ServerEvent → State Mapping

```
ServerEvent::TextDelta { delta }
  → append delta to streaming buffer of last AssistantMessage
  → trigger ChatView re-render

ServerEvent::ToolCallStarted { call_id, name, arguments }
  → create ToolCallWidget with pending border
  → append to current AssistantMessage's tool_calls

ServerEvent::ToolCallDelta { call_id, delta }
  → append to tool call's streaming argument display

ServerEvent::ToolCallDone { call_id, result, is_error }
  → set final result, swap border color (success/error)
  → collapse if all tools for this message done

ServerEvent::TurnEnd { stop_reason, usage }
  → finalize streaming message
  → update StatusBar token counts
  → transition AppState::Busy → Connected

ServerEvent::Error { code, message }
  → show inline error banner in ChatView
  → transition AppState::Busy → Connected
```

### 7.3 HTTP POST Flow

```
InputBar::submit(text)
  → if text starts with '/': dispatch to Command::parse()
  → else:
      1. Expand any [paste markers] in text
      2. POST /api/v1/sessions/{id}/messages { content: text }
      3. Append UserMessage to State
      4. Transition to Busy (SSE events expected shortly)
      5. Clear input buffer, push to history stack
```

---

## 8. Markdown Rendering Pipeline

```
raw markdown string
  → pulldown_cmark::Parser  (event iterator)
  → stateful event → Text<'static> conversion:
      - Paragraph: TextStyle::Body
      - Heading(1..=6): TextStyle::Heading with size-based scaling
      - Code(Fenced): style via syntect if language tag present; fallback to unstyled preformatted block if no language tag
      - BlockQuote: prefix "│ " + italic
      - List(Ordered/Unordered): proper indentation, bullet/number
      - InlineCode: TextStyle::Code (inverted or accent bg)
      - Bold/Italic/Strikethrough: ratatui Modifier flags
      - Link: render as "text (url)" (no hyperlink in terminal)
  → Line::styled() vectors → ratatui Paragraph
```

Theme functions map semantic style names to ratatui `Style` values.

---

## 9. Paste Handling

Leverage crossterm's bracketed paste mode (`EnableBracketedPaste` on terminal setup):

```
Paste event received
  → if lines exceeded > 10:
      marker_id = paste_store.insert(full_content)
      input_buffer += format!("[paste #{marker_id} +{n} lines]")
  → else:
      input_buffer += pasted_text (trim trailing newline)

On submit:
  → resolve any [paste #N ...] markers to full content
```

---

## 10. Command System

```rust
enum Command {
    Quit,
    NewSession { title: Option<String> },
    SwitchSession { id: String },
    ListSessions,
    SelectModel { id: Option<String> },
    Clear,
    Help,
    Export { path: String },       // placeholder: writes session transcript to file as JSON
    Connect { url: String },
    Auth { token: String },
    Tokens,
}
```

Parsed from `/command [args...]` prefix in input bar. Commands that require server interaction dispatch HTTP requests through `rest.rs`.

---

## 11. Configuration

```toml
# ~/.config/pandaria/tui/config.toml
[server]
url = "http://localhost:8080"
timeout_secs = 30

[auth]
token = "${PANDARIA_TOKEN}"    # env var interpolation

[ui]
max_history = 500
show_tool_calls = true
syntax_theme = "base16-ocean.dark"
scrollback = 1000

[keys]
# User keybinding overrides (TBD MVP)
```

Priority: CLI flags (`--url`, `--token`, `--theme`) > environment variables (`PANDARIA_URL`, `PANDARIA_TOKEN`) > config file > built-in defaults.

---

## 12. Error Handling

| Error Category | User-Visible | Handling |
|---|---|---|
| Connection refused | StatusBar: "Disconnected · retrying..." | Exponential backoff, max 5 retries, then give up |
| Auth failure (401) | StatusBar: "Auth failed. Run /auth <token>" | Stop retries, prompt user |
| SSE stream break | ChatView banner: "Connection lost. Reconnecting..." | Auto-reconnect, replay last seen event ID |
| HTTP timeout | StatusBar: "Request timed out" | Retry once, then surface |
| Invalid command | InputBar flash: "Unknown command: /xyz" | Red text for 1s |
| API error (4xx/5xx) | ChatView inline error: "[Server] <message>" | No retry on 4xx; 5xx retry once |
| Render overflow | Log to crash file, attempt clean exit | `~/.config/pandaria/tui/crash.log` |

All errors via `thiserror::Error`. No `.unwrap()` outside tests.

---

## 13. Testing Strategy

| Layer | What | How |
|---|---|---|
| `command.rs` | Command parser round-trip | Unit tests: `/switch abc → Command::SwitchSession("abc")` |
| `markdown.rs` | Markdown → styled text | Unit tests: assert headings produce `Modifier::BOLD`, code blocks get `Style::code()` |
| `paste.rs` | Paste marker creation/resolution | Unit tests: paste 15 lines → marker, resolve → original |
| `client/model.rs` | JSON serde round-trip | Unit tests: `ServerEvent` variants serialize/deserialize correctly |
| `state.rs` | Message append, tool call lifecycle | Unit tests: Stream TextDelta → appended; ToolCallStarted → widget created |
| `ui/layout.rs` | Layout splits are valid | Unit tests: ensure no widget overflows terminal area at minimum 80×24 |
| Integration | Full app startup + mock HTTP server | `tests/integration.rs` using `wiremock` to simulate API responses |

`cargo test -p tui` must pass all without external network.

---

## 14. Key Design Decisions

| Decision | Rationale |
|---|---|
| ratatui over cursive | Lower-level control needed for differential rendering, overlay compositing, precise cursor management. More active community. |
| SSE over WebSocket for MVP | Simpler to implement, standard `text/event-stream`, aligns with how LLM streaming APIs already work. Upgrade to WS later if bidirectional control needed. |
| Separate binary crate, not a feature flag | The TUI has fundamentally different dependency tree (ratatui, crossterm, syntect) and should not bloat the server binary. Clear separation of concerns. |
| Overlay compositing, not replacement | pi.dev proved this pattern: overlays composited on top preserve context and avoid full redraws. Modal replacement would lose scroll position. |
| Large paste markers | pi.dev's solution to bracketed paste flooding. Prevents TUI frame drops from rendering giant pastes inline. |
| Tool calls inline, not in a side panel | Space-constrained terminal. Side panels split attention. Inline with expand/collapse keeps flow natural like a chat log. |
| State per session in HashMap | Users switch between sessions. Keeping all session states in memory avoids re-fetch on every tab switch. MVP cap: 10 sessions. |
| HTTP POST + SSE, not bidirectional WS | The interaction model is asymmetric: client sends one message, receives many events. SSE is the natural fit. |

---

## 15. Out of Scope (MVP)

- Image rendering (Kitty/iTerm2 protocol)
- WASM/plugin-based extensions in the TUI
- Multi-tenant management UI (this is a client, not admin panel)
- Session persistence on the client side (server handles this)
- Streaming tool output (displaying tool results as they stream in)
- IME composition window support
- Terminal multiplexer (tmux) resize detection beyond SIGWINCH
