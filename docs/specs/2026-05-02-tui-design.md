# Pandaria TUI Module — Specification

**Date:** 2026-05-03
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
    keybindings.rs           # KeybindingsManager: global keybinding registry, conflict detection, user overrides
    autocomplete.rs          # AutocompleteProvider trait, SlashCommandProvider, FilePathProvider
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
      header.rs              # HeaderBar
      chat_view.rs           # ChatView: messages, separators, error banners
      editor.rs              # Editor: multi-line text input with Emacs keybindings, autocomplete, bracketed paste
      tool_call.rs           # Expandable/collapsible tool call display
      thinking.rs            # ThinkingBlock: collapsible reasoning display
      spinner.rs             # SpinnerWidget: animated frame rotation
      status_bar.rs          # Connection status, context usage gauge, spinner slot
      session_tabs.rs        # Session switcher bar
    overlays/
      mod.rs                 # OverlayStack: push/pop/clear, focus routing, compositing
      command_palette.rs     # Command selector (/ prefix dispatch)
      session_list.rs        # Session browser overlay
      help.rs                # Keybinding reference
      autocomplete.rs        # AutocompleteOverlay: suggestion list, fuzzy filter
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
eventsource-stream = "0.2"
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

`AppState` governs interaction behavior (what inputs are accepted). `ConnectionStatus` (in `State`) governs network health display and auto-reconnect logic. They are orthogonal: `AppState::Connected` + `ConnectionStatus::Reconnecting` means the UI is interactive but the SSE stream is restarting.

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
    /// Current inline error, if any. Displayed as a banner in ChatView.
    /// Cleared at the start of each new turn.
    error: Option<ApiError>,
}

/// A message ready to be displayed in the ChatView.
struct RenderedMessage {
    role: MessageRole,            // User, Assistant
    blocks: Vec<MessageBlock>,    // ordered content blocks (text, tool call, thinking, markdown)
    timestamp: SystemTime,
    /// Message lifecycle state. Updated in-place during streaming.
    status: MessageStatus,
}

enum MessageStatus {
    Streaming,    // deltas still arriving; spinner shown in StatusBar
    Complete,     // fully received, no more deltas
    Aborted,      // user interrupted (Escape), show dimmed
    Error,        // SSE error received, show red border + error message in content
}

enum MessageBlock {
    Text(Vec<Line<'static>>),     // rendered ratatui lines
    ToolCall(ToolCallWidget),
    Thinking(ThinkingBlock),       // collapsible reasoning display
}

/// Expandable/collapsible tool call display block.
struct ToolCallWidget {
    call_id: String,
    name: String,
    /// Visual state: Pending (border = yellow), Success (green), Error (red)
    state: ToolCallState,
    /// Partial argument display during streaming; final result after completion
    content: Vec<Line<'static>>,
    is_expanded: bool,
}

enum ToolCallState {
    Pending,
    Success,
    Error,
}

/// Collapsible thinking / extended reasoning display.
/// Inspired by pi.dev's treatment of ThinkingContent as a first-class block.
struct ThinkingBlock {
    thinking_text: String,
    is_expanded: bool,             // user-toggled; defaults to collapsed
    is_redacted: bool,             // Anthropic redacted_thinking
}

/// Accumulates streaming deltas for the in-flight AssistantMessage.
struct StreamingBuffer {
    text_content: String,               // accumulated text so far
    thinking_content: String,           // accumulated thinking/reasoning text
    pending_tool_calls: Vec<ToolCallWidget>,  // tool calls seen in this turn
    /// Per-call_id argument accumulation buffer for streaming tool arguments.
    tool_arg_buffers: HashMap<String, String>,  // call_id → partial_json
}

/// Network-level connection health, tracked independently of App's
/// interaction state machine (Disconnected/Connected/Busy).
enum ConnectionStatus {
    Disconnected,
    Connected,
    Reconnecting,
}
```

### 6.3 Widgets

| Widget | Ratatui Primitive | Key Behavior |
|---|---|---|
| `HeaderBar` | `Paragraph` | Session name, model, connection icon |
| `SessionTabs` | `Tabs` | Active highlight, close button (X), new-session (+). When tabs exceed terminal width, show last visible tab as `…` indicating more. |
| `ChatView` | `Paragraph` in scrollable region | Append-only, auto-scroll to bottom unless user scrolled up. **User messages**: accent-tinted background. **Assistant messages**: Aborted → dimmed all content. Error → red left border + error text. **Separators**: thin horizontal divider inserted after finishing a batch of tool calls (ToolCallDone when pending_tool_calls exhausted for the current message). Purely-visual message with no tool calls do not get separators. |
| `ToolCallWidget` | `Paragraph` + `Block` borders | Expand/collapse toggle, pending/success/error border color |
| `ThinkingBlock` | `Paragraph` + `Block` borders | Collapsible reasoning display. Collapsed: single line `💭 Thinking...`. Expanded: full text + separator. Toggle: `Enter` on focused block or `Ctrl+T` toggles all. |
| `SpinnerWidget` | `Paragraph` (animated frames) | Rotating frame animation rendered in StatusBar. Cycles through `[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]` at 80ms interval. Visible only when `AppState::Busy`. Frame advancement via `tokio::select!` ticker. |
| `StatusBar` | `Paragraph` + `Gauge` | **Left**: connection status icon (`●` green = connected, `○` grey = disconnected, `↻` yellow = reconnecting). **Center**: spinner widget (when busy). **Right**: context usage gauge `[████░░░░] 45%` + model name. Gauge computed from `usage.input_tokens / context_window` (context_window from session info). |
| `Editor` | Custom multi-line buffer | Multi-line text input with Emacs keybindings, vertical scrolling when content exceeds viewport, bracketed paste support, autocomplete trigger. Replaces single-line `InputBar`. |

### 6.4 Overlays

| Overlay | Trigger | Behavior |
|---|---|---|
| `CommandPalette` | `/` in input | Fuzzy-filter command list, Enter dispatches |
| `SessionList` | `/list` or `Ctrl+S` | Selectable session list, Enter switches, Delete removes |
| `Help` | `/help` or `F1` | Scrollable keybinding reference, Escape dismisses |
| `ModelSelector` | `/model` or `Ctrl+L` | Selectable model list, Enter confirms |
| `AutocompleteOverlay` | `/` or `Tab` in editor | Fuzzy-filtered suggestion list for slash commands and file paths. Arrow keys navigate, Enter confirms, Escape cancels. |

Overlay compositing rules:

- **Positioning**: Anchored to center of terminal by default. Configurable anchors: `Center`, `TopLeft`, `TopRight`, `BottomLeft`, `BottomRight`.
- **Sizing**: Width = `min(content_width + 4, terminal_width - 4)`. Height = content line count (bounded by `terminal_height - 4`). Content clipping if too large.
- **Rendering**: During `Frame::render_widget()`, overlay widget renders to a temporary `Buffer`. Lines are then spliced into the frame buffer at the calculated row/col using `Buffer::set_span()`. Base content behind the overlay is dimmed by applying `Modifier::DIM` to existing cells before overlay painting.
- **Focus**: Only the top-most overlay receives keyboard input. Input is routed through `App::handle_input()` checking `overlay_stack.top()` before other widgets.
- **Dismissal**: `Escape` dismisses the top overlay. Non-capturing overlays (e.g., HelpView) also dismiss on any char input.

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

All keyboard input is dispatched through `KeybindingsManager`, which maps `crossterm::KeyEvent` to semantic `Keybinding` identifiers. User overrides from `config.toml` take precedence over built-in defaults.

```
app.quit                → quit application (unless streaming)
app.interrupt           → if streaming: send HTTP DELETE interrupt; else: clear editor
app.toggle_tool_calls   → toggle all tool calls expanded/collapsed
app.toggle_thinking     → toggle all thinking blocks expanded/collapsed
app.select_model        → open model selector overlay
app.list_sessions       → open session list overlay
app.open_command_palette → open command palette overlay

editor.submit           → if overlay focused: confirm overlay action
                          else if autocomplete visible: confirm suggestion
                          else: submit editor text as UserMessage
editor.new_line         → insert newline (Shift+Enter, Alt+Enter)
editor.cursor_up        → if overlay focused: navigate overlay list
                          else if cursor on first line and line empty: history previous
                          else: move cursor up one line
editor.cursor_down      → if overlay focused: navigate overlay list
                          else if cursor on last line: history next
                          else: move cursor down one line
editor.cursor_left      → move cursor left
editor.cursor_right     → move cursor right
editor.cursor_word_left → move cursor to previous word boundary
editor.cursor_word_right → move cursor to next word boundary
editor.cursor_line_start → move cursor to start of line
editor.cursor_line_end  → move cursor to end of line
editor.page_up          → scroll editor viewport up one page
editor.page_down        → scroll editor viewport down one page
editor.delete_char_backward → delete character before cursor
editor.delete_char_forward  → delete character after cursor
editor.delete_word_backward → delete word before cursor (kill)
editor.delete_word_forward  → delete word after cursor
editor.delete_to_line_start → delete from cursor to start of line
editor.delete_to_line_end   → delete from cursor to end of line (kill)
editor.yank             → yank (paste) from kill ring
editor.yank_pop         → cycle through kill ring
editor.undo             → undo last edit
editor.history_prev     → recall previous history entry
editor.history_next     → recall next history entry

autocomplete.trigger    → trigger autocomplete (Tab)

Char('/')               → if editor empty: open command palette
Char(ch)                → insert character into editor
```

**Default Keybindings Table:**

| Keybinding | Default Keys | Description |
|---|---|---|
| `app.quit` | `ctrl+c`, `ctrl+d` | Quit application |
| `app.interrupt` | `escape` | Interrupt streaming or clear editor |
| `app.toggle_tool_calls` | `ctrl+o` | Toggle tool calls visibility |
| `app.toggle_thinking` | `ctrl+t` | Toggle thinking blocks visibility |
| `app.select_model` | `ctrl+l` | Open model selector |
| `app.list_sessions` | `ctrl+s` | Open session list |
| `app.open_command_palette` | `/` (when editor empty) | Open command palette |
| `editor.submit` | `enter` | Submit message |
| `editor.new_line` | `shift+enter`, `alt+enter` | Insert newline |
| `editor.cursor_up` | `up` | Move cursor up |
| `editor.cursor_down` | `down` | Move cursor down |
| `editor.cursor_left` | `left`, `ctrl+b` | Move cursor left |
| `editor.cursor_right` | `right`, `ctrl+f` | Move cursor right |
| `editor.cursor_word_left` | `alt+left`, `ctrl+left`, `alt+b` | Previous word |
| `editor.cursor_word_right` | `alt+right`, `ctrl+right`, `alt+f` | Next word |
| `editor.cursor_line_start` | `home`, `ctrl+a` | Line start |
| `editor.cursor_line_end` | `end`, `ctrl+e` | Line end |
| `editor.page_up` | `pageup` | Page up |
| `editor.page_down` | `pagedown` | Page down |
| `editor.delete_char_backward` | `backspace` | Delete backward |
| `editor.delete_char_forward` | `delete`, `ctrl+d` | Delete forward |
| `editor.delete_word_backward` | `ctrl+w`, `alt+backspace` | Delete word backward |
| `editor.delete_word_forward` | `alt+d`, `alt+delete` | Delete word forward |
| `editor.delete_to_line_start` | `ctrl+u` | Delete to line start |
| `editor.delete_to_line_end` | `ctrl+k` | Delete to line end |
| `editor.yank` | `ctrl+y` | Yank from kill ring |
| `editor.yank_pop` | `alt+y` | Cycle kill ring |
| `editor.undo` | `ctrl+-` | Undo |
| `editor.history_prev` | `up` (on first empty line) | Previous history |
| `editor.history_next` | `down` (on last line) | Next history |
| `autocomplete.trigger` | `tab` | Trigger autocomplete |

### 7.2 ServerEvent → State Mapping

```
ServerEvent::MessageStart { message_index }
  → create new RenderedMessage with status = Streaming
  → append to active session's messages

ServerEvent::TextDelta { delta }
  → append delta to StreamingBuffer.text_content
  → re-render last message's Text block with accumulated content
  → trigger ChatView re-render

ServerEvent::ThinkingDelta { content_index, delta }
  → append delta to StreamingBuffer.thinking_content
  → update or create ThinkingBlock in last message's blocks
  → ThinkingBlock defaults collapsed; expanded if user toggled previously
  → Note: `content_index` is reserved for future multi-block support; v0.1
    uses a single thinking block per message (content_index = 0)

ServerEvent::ToolCallStarted { call_id, name }
  → create ToolCallWidget with pending border (no arguments yet)
  → append to current AssistantMessage's blocks as MessageBlock::ToolCall

ServerEvent::ToolCallDelta { call_id, delta }
  → append delta to StreamingBuffer.tool_arg_buffers[call_id]
  → re-render tool call widget with accumulated partial arguments

ServerEvent::ToolCallDone { call_id, result, is_error }
  → set final result, swap border color (success/error)
  → if the tool call was the last in this message, insert a separator line after it in ChatView

ServerEvent::TurnEnd { stop_reason, usage }
  → finalize streaming message: status = Complete
  → update StatusBar token counts and context usage gauge
  → transition AppState::Busy → Connected

ServerEvent::Error { code, message }
  → set current streaming message status = Error
  → store error text in SessionState.error
  → show inline error banner in ChatView with red left border
  → transition AppState::Busy → Connected

Server-initiated abort (no dedicated event):
  → When user presses Escape during streaming, HTTP DELETE is sent to server.
    Server responds by closing the SSE stream (no new events).
    Client optimistically sets current streaming message status = Aborted
    immediately upon sending the DELETE request (before server acknowledgment).
  → Aborted messages render all accumulated blocks dimmed.
```

### 7.3 HTTP POST Flow

```
Editor::submit()
  → text = editor.take_text()
  → expand any [paste #N +n lines] markers to full content via PasteStore
  → if text starts with '/': dispatch to Command::parse()
  → else:
      1. POST /api/v1/sessions/{id}/messages { content: text }
      2. Append UserMessage to State
      3. Transition to Busy (SSE events expected shortly)
      4. Push text to editor history stack
```

**Editor take_text() behavior:**
- Joins all lines with `\n` separators
- Resolves paste markers: `[paste #1 +23 lines]` → original content from PasteStore
- Clears editor content (lines = `[""]`, cursor = (0, 0))
- Triggers re-render

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
      - Link: render as "text (url)". Long URLs (>60 chars) are truncated
        with ellipsis to prevent line overflow. No OSC 8 hyperlinks in v0.1.
  → Line::styled() vectors → ratatui Paragraph
```

Theme functions map semantic style names to ratatui `Style` values.

---

## 9. Paste Handling

Leverage crossterm's bracketed paste mode (`EnableBracketedPaste` on terminal setup):

### 9.1 Terminal Setup

```rust
// In main.rs terminal initialization
stdout.execute(EnableBracketedPaste)?;
```

This enables the terminal to wrap pasted content in bracketed paste sequences (`\x1b[200~` ... `\x1b[201~`), allowing the application to distinguish typed input from pasted content.

### 9.2 Paste Event Processing

```
Event::Paste(content) received from crossterm
  → if content.lines().count() > 10:
      marker_id = paste_store.insert(full_content)
      editor.insert_paste_marker(marker_id, line_count)
      // Editor displays: [paste #1 +23 lines]
  → else:
      editor.insert_text(content)
      // Content inserted directly at cursor position
```

### 9.3 PasteStore

```rust
pub struct PasteStore {
    counter: usize,
    storage: HashMap<usize, String>,
}

impl PasteStore {
    pub fn insert(&mut self, content: String) -> usize;
    pub fn get(&self, id: usize) -> Option<&str>;
    pub fn resolve_markers(&self, text: &str) -> String;
}
```

**Marker format:** `[paste #1 +23 lines]`
- `Regex`: `\[paste #(\d+)( \+(\d+) lines)?\]`
- Only markers with valid IDs in `PasteStore` are expanded
- Literal `[paste #N ...]` text in user input is preserved (no expansion) if ID not found

### 9.4 Marker Expansion on Submit

When `editor.take_text()` is called:

```
1. Join all editor lines with \n
2. Scan for paste marker regex matches
3. For each match:
   - Look up marker_id in PasteStore
   - If found: replace marker with stored content
   - If not found: leave marker as-is (literal text)
4. Return expanded text
```

### 9.5 Large Paste Handling

Pasted content exceeding 10 lines is folded into a marker to:
- Prevent TUI frame drops from rendering giant text blocks
- Keep the editor viewport manageable
- Preserve the full content for message submission

Users can manually type `[paste #N ...]` markers if they know valid IDs, though this is not a supported workflow.

---

## 10. Editor Widget (src/widgets/editor.rs)

The `Editor` widget replaces the single-line `InputBar` with a full multi-line text editor. It supports Emacs-style keybindings, vertical scrolling, bracketed paste, and autocomplete triggering.

### 10.1 Structure

```rust
pub struct Editor {
    lines: Vec<String>,              // Multi-line content (each line without \n)
    cursor_line: usize,              // Cursor row index
    cursor_col: usize,               // Cursor column (char index, grapheme-aware)
    preferred_col: Option<usize>,    // Preferred column for Up/Down navigation
    viewport_top: usize,             // First visible line index
    history: Vec<String>,            // Submitted text history
    history_index: Option<usize>,    // None = not navigating history
    paste_markers: HashMap<usize, String>, // marker_id → full_content
    is_pasting: bool,                // Bracketed paste state
    pending_paste: String,           // Paste buffer
    kill_ring: Vec<String>,          // Emacs kill ring (yank history)
    last_kill_appended: bool,        // Whether last kill was appended to ring
    undo_stack: Vec<EditorState>,    // Undo history
}

#[derive(Clone)]
struct EditorState {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}
```

### 10.2 Cursor Movement

All cursor operations are grapheme-aware (using Unicode segmenter):

| Method | Keybinding | Behavior |
|---|---|---|
| `cursor_up()` | `editor.cursor_up` | Move to previous line; maintain `preferred_col` |
| `cursor_down()` | `editor.cursor_down` | Move to next line; maintain `preferred_col` |
| `cursor_left()` | `editor.cursor_left` | Move one grapheme left |
| `cursor_right()` | `editor.cursor_right` | Move one grapheme right |
| `cursor_word_left()` | `editor.cursor_word_left` | Move to start of previous word |
| `cursor_word_right()` | `editor.cursor_word_right` | Move to end of next word |
| `cursor_line_start()` | `editor.cursor_line_start` | Move to column 0 |
| `cursor_line_end()` | `editor.cursor_line_end` | Move to end of current line |
| `cursor_doc_start()` | `ctrl+home` | Move to start of document |
| `cursor_doc_end()` | `ctrl+end` | Move to end of document |
| `page_up()` | `editor.page_up` | Scroll viewport up, move cursor |
| `page_down()` | `editor.page_down` | Scroll viewport down, move cursor |

**Word boundary definition:** A "word" is a contiguous sequence of alphanumeric characters or underscores. All other characters (including whitespace and punctuation) are treated as separate single-character words. This matches Emacs behavior and works correctly for mixed CJK/Latin text.

### 10.3 Insert & Delete

| Method | Keybinding | Behavior |
|---|---|---|
| `insert_char(ch)` | `Char(ch)` | Insert character at cursor |
| `insert_newline()` | `editor.new_line` | Split line at cursor |
| `delete_char_backward()` | `editor.delete_char_backward` | Delete grapheme before cursor |
| `delete_char_forward()` | `editor.delete_char_forward` | Delete grapheme after cursor |
| `delete_word_backward()` | `editor.delete_word_backward` | Delete word before cursor; push to kill ring |
| `delete_word_forward()` | `editor.delete_word_forward` | Delete word after cursor; push to kill ring |
| `delete_to_line_start()` | `editor.delete_to_line_start` | Delete from cursor to start; push to kill ring |
| `delete_to_line_end()` | `editor.delete_to_line_end` | Delete from cursor to end; push to kill ring |
| `kill_ring_yank()` | `editor.yank` | Paste most recent kill |
| `kill_ring_yank_pop()` | `editor.yank_pop` | Cycle to next kill in ring |
| `undo()` | `editor.undo` | Revert to previous EditorState |

**Kill ring behavior:**
- Consecutive kills at the same position are appended to the same kill ring entry
- `yank` inserts the most recent kill
- `yank_pop` replaces the yanked text with the next entry in the ring

### 10.4 History Navigation

History only triggers when cursor is at the boundary:

- **Up arrow on first line, empty or at column 0**: recall previous history entry
- **Down arrow on last line**: recall next history entry
- History entries are full multi-line strings (joined with `\n`)
- Restoring a history entry replaces all editor lines
- Any modification after history recall resets `history_index` to `None`

### 10.5 Bracketed Paste Handling

```
Event::Paste(content)
  → if content.lines().count() > 10:
       marker_id = paste_store.insert(content)
       editor.insert_paste_marker(marker_id, line_count)
       // e.g., [paste #1 +23 lines]
  → else:
       editor.insert_text(content)
```

**Paste marker expansion** (in `take_text()`):
```
Regex: \[paste #(\d+)( \+(\d+) lines)?\]
Replace with: paste_store.get(id).unwrap_or("")
```

### 10.6 Autocomplete Trigger

The editor provides context to autocomplete providers:

```rust
pub fn text_before_cursor(&self) -> String;  // All text from start to cursor
pub fn current_line_text(&self) -> &str;     // Full current line
```

Providers inspect this context to decide whether to show suggestions.

### 10.7 Rendering

```
┌─────────────────────────────────────────┐  ← top border (Borders::TOP)
│ First line of user input                │
│ Second line with more text █            │  ← cursor shown as block
│                                         │
│ [paste #1 +23 lines]                    │
│                                         │
└─────────────────────────────────────────┘
```

- **Height**: minimum 1 line, maximum 40% of terminal height
- **Scroll**: `viewport_top` tracks first visible line; auto-scrolls to cursor
- **Placeholder**: shown when empty and not focused: "Write a message or /command..."
- **Busy state**: placeholder changes to "Interrupt (Esc)…"
- **Cursor**: rendered as inverse-color block at cursor position

---

## 11. Keybindings System (src/keybindings.rs)

### 11.1 Architecture

```rust
pub type KeyId = String;
// Format examples: "enter", "escape", "ctrl+c", "shift+enter", "alt+left"

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keybinding {
    AppQuit, AppInterrupt, AppToggleToolCalls, AppToggleThinking,
    AppSelectModel, AppListSessions, AppOpenCommandPalette,
    EditorCursorUp, EditorCursorDown, EditorCursorLeft, EditorCursorRight,
    EditorCursorWordLeft, EditorCursorWordRight,
    EditorCursorLineStart, EditorCursorLineEnd,
    EditorCursorDocStart, EditorCursorDocEnd,
    EditorPageUp, EditorPageDown,
    EditorDeleteCharBackward, EditorDeleteCharForward,
    EditorDeleteWordBackward, EditorDeleteWordForward,
    EditorDeleteToLineStart, EditorDeleteToLineEnd,
    EditorNewLine, EditorSubmit, EditorUndo,
    EditorYank, EditorYankPop,
    EditorHistoryPrev, EditorHistoryNext,
    AutocompleteTrigger,
}

pub struct KeybindingsManager {
    defaults: HashMap<Keybinding, Vec<KeyId>>,
    user: HashMap<Keybinding, Vec<KeyId>>,
    resolved: HashMap<Keybinding, Vec<KeyId>>,
}
```

### 11.2 KeyEvent → KeyId Conversion

```
KeyEvent { code: Char('c'), modifiers: CONTROL } → "ctrl+c"
KeyEvent { code: Enter, modifiers: SHIFT }        → "shift+enter"
KeyEvent { code: Left, modifiers: ALT }           → "alt+left"
KeyEvent { code: Char('p'), modifiers: CONTROL | SHIFT } → "ctrl+shift+p"
```

Rules:
1. Modifiers sorted alphabetically: `alt`, `ctrl`, `shift`
2. Special keys use lowercase name: `enter`, `escape`, `backspace`, `tab`, `delete`, `home`, `end`, `pageup`, `pagedown`, `up`, `down`, `left`, `right`
3. Character keys use lowercase: `a`, `1`, `/`

### 11.3 Configuration Format

```toml
[keys]
"app.quit" = ["ctrl+c", "ctrl+d"]
"app.interrupt" = "escape"
"editor.cursor_word_left" = ["alt+left", "ctrl+left", "alt+b"]
"editor.delete_word_backward" = ["ctrl+w", "alt+backspace"]
"editor.new_line" = ["shift+enter", "alt+enter"]
```

TOML values can be a single string or an array of strings.

### 11.4 Conflict Detection

The manager detects when multiple keybindings share the same KeyId. Conflicts are logged as warnings but do not prevent operation (last-defined wins in resolution).

---

## 12. Autocomplete System (src/autocomplete.rs)

### 12.1 Provider Trait

```rust
pub trait AutocompleteProvider: Send + Sync {
    fn should_trigger(&self, context: &AutocompleteContext) -> bool;
    fn get_suggestions(&self, context: &AutocompleteContext) -> Vec<Suggestion>;
}

pub struct AutocompleteContext {
    pub full_text: String,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub current_line: String,
    pub text_before_cursor: String,
}

pub struct Suggestion {
    pub label: String,
    pub value: String,
    pub description: Option<String>,
}
```

### 12.2 SlashCommandProvider

Triggered when current line starts with `/` and cursor is after the `/`.

```rust
pub struct SlashCommandProvider {
    commands: Vec<SlashCommand>,
}

pub struct SlashCommand {
    pub name: String,
    pub description: String,
}
```

**Fuzzy matching:** Uses substring prefix match on command names. Results sorted alphabetically.

### 12.3 FilePathProvider

Triggered by `Tab` key (explicit) or when typing a path separator.

```rust
pub struct FilePathProvider {
    base_dir: PathBuf,
    fd_available: bool,  // Runtime detection
}
```

**File search strategy:**
1. **Primary (fd available):** Spawn `fd` process with query to get fuzzy file matches. Respects `.gitignore`.
2. **Fallback (fd unavailable):** Use `std::fs::read_dir()` for directory listing with simple prefix matching.

**Path formats supported:**
- Absolute: `/home/user/...`
- Home-relative: `~/...`
- Current-relative: `./...`, `../...`
- Bare filename: `file.txt`

### 12.4 AutocompleteOverlay

Renders as a floating list above the editor:

```
┌─────────────────────────────────────────┐
│ > /cle                                  │  ← editor
│ ┌── Suggestions ───────────────┐        │  ← overlay
│ │  clear    Clear all messages │        │
│ │▶ clear    Clear all messages │        │  ← selected
│ │  delete   Delete last message│        │
│ └──────────────────────────────┘        │
└─────────────────────────────────────────┘
```

- Position: bottom-left aligned to editor cursor position
- Max height: 8 lines
- Max width: 60 columns
- Navigation: Up/Down arrows, Enter to confirm, Escape to cancel
- Confirmed suggestion replaces the prefix in the editor

---

## 13. Command System

```rust
enum Command {
    Quit,
    NewSession { title: Option<String> },
    SwitchSession { id: String },
    ListSessions,
    SelectModel { id: Option<String> },
    Clear,
    Help,
    Connect { url: String },
    Auth { token: String },
    Tokens,
}
```

Parsed from `/command [args...]` prefix in editor. Commands that require server interaction dispatch HTTP requests through `rest.rs`.

---

## 14. Configuration

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
# User keybinding overrides
# Single key or array of keys per binding
"app.quit" = ["ctrl+c", "ctrl+d"]
"app.interrupt" = "escape"
"editor.cursor_word_left" = ["alt+left", "ctrl+left", "alt+b"]
"editor.delete_word_backward" = ["ctrl+w", "alt+backspace"]
"editor.new_line" = ["shift+enter", "alt+enter"]
```

Priority: CLI flags (`--url`, `--token`, `--theme`) > environment variables (`PANDARIA_URL`, `PANDARIA_TOKEN`) > config file > built-in defaults.

### 14.1 KeysConfig Structure

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeysConfig {
    #[serde(flatten)]
    pub bindings: HashMap<String, toml::Value>,
    // e.g., "editor.cursor_left" → Value::String("left")
    //       "editor.cursor_word_left" → Value::Array(["alt+left", "ctrl+b"])
}
```

TOML parsing uses `serde(flatten)` to allow arbitrary keybinding keys under `[keys]`. Values are parsed as either a single string or an array of strings, normalized to `Vec<String>` during loading.

---

## 15. Error Handling

| Error Category | User-Visible | Handling |
|---|---|---|
| Connection refused | StatusBar: "Disconnected · retrying..." | Exponential backoff, max 5 retries, then give up |
| Auth failure (401) | StatusBar: "Auth failed. Run /auth <token>" | Stop retries, prompt user |
| SSE stream break | ChatView banner: "Connection lost. Reconnecting..." | Auto-reconnect, replay last seen event ID |
| HTTP timeout | StatusBar: "Request timed out" | Retry once, then surface |
| Invalid command | Editor flash: "Unknown command: /xyz" | Red text for 1s |
| API error (4xx/5xx) | ChatView inline error: "[Server] <message>" | No retry on 4xx; 5xx retry once |
| Render overflow | Log to crash file, attempt clean exit | `~/.config/pandaria/tui/crash.log` |

All errors via `thiserror::Error`. No `.unwrap()` outside tests.

---

## 16. Testing Strategy

| Layer | What | How |
|---|---|---|
| `command.rs` | Command parser round-trip | Unit tests: `/switch abc → Command::SwitchSession("abc")` |
| `markdown.rs` | Markdown → styled text | Unit tests: assert headings produce `Modifier::BOLD`, code blocks get `Style::code()` |
| `widgets/thinking.rs` | ThinkingBlock expand/collapse | Unit tests: collapsed → 1 line; expanded → full text; toggle transitions |
| `widgets/spinner.rs` | Spinner frame cycling | Unit tests: 8 frames cycle in order at 80ms interval via ticker |
| `paste.rs` | Paste marker creation/resolution | Unit tests: paste 15 lines → marker, resolve → original |
| `client/model.rs` | JSON serde round-trip | Unit tests: `ServerEvent` variants serialize/deserialize correctly |
| `state.rs` | Message append, tool call lifecycle | Unit tests: Stream TextDelta → appended; MessageStatus transitions |
| `state.rs` | Out-of-order SSE events | Unit tests: ToolCallDone before ToolCallDelta → no panic, shows tool with empty result |
| `ui/layout.rs` | Layout splits are valid | Unit tests: ensure no widget overflows terminal area at minimum 80×24 |
| `keybindings.rs` | KeyEvent → KeyId conversion | Unit tests: `ctrl+c`, `shift+enter`, `alt+left` parsing |
| `keybindings.rs` | Keybinding matching | Unit tests: verify all default keybindings match correctly |
| `widgets/editor.rs` | Cursor movement | Unit tests: Up/Down maintain preferred_col; word left/right boundaries |
| `widgets/editor.rs` | Kill ring | Unit tests: kill → yank → yank_pop cycle |
| `widgets/editor.rs` | History navigation | Unit tests: recall previous entry, modify resets index |
| `widgets/editor.rs` | Paste handling | Unit tests: bracketed paste → marker → expand |
| `autocomplete.rs` | Slash command fuzzy match | Unit tests: `/cl` matches `clear` |
| `autocomplete.rs` | File path completion | Unit tests: `src/` returns directory contents |
| Integration | Full app startup + mock HTTP server | `tests/integration.rs` using `wiremock` to simulate API responses |

`cargo test -p tui` must pass all without external network.

---

## 17. Key Design Decisions

| Decision | Rationale |
|---|---|---|
| ratatui over cursive | Lower-level control needed for differential rendering, overlay compositing, precise cursor management. More active community. |
| SSE over WebSocket for MVP | Simpler to implement, standard `text/event-stream`, aligns with how LLM streaming APIs already work. Upgrade to WS later if bidirectional control needed. |
| Separate binary crate, not a feature flag | The TUI has fundamentally different dependency tree (ratatui, crossterm, syntect) and should not bloat the server binary. Clear separation of concerns. |
| Overlay compositing, not replacement | pi.dev proved this pattern: overlays composited on top preserve context and avoid full redraws. Modal replacement would lose scroll position. |
| Large paste markers | pi.dev's solution to bracketed paste flooding. Prevents TUI frame drops from rendering giant pastes inline. |
| Tool calls inline, not in a side panel | Space-constrained terminal. Side panels split attention. Inline with expand/collapse keeps flow natural like a chat log. |
| State per session in HashMap | Users switch between sessions. Keeping all session states in memory avoids re-fetch on every tab switch. MVP cap: 10 sessions. |
| HTTP POST + SSE, not bidirectional WS | The interaction model is asymmetric: client sends one message, receives many events. SSE is the natural fit. |
| Thinking as first-class block | pi.dev treats ThinkingContent as an independent content block alongside text and tool calls. Collapsible reasoning display preserves context without cluttering the chat. |
| Message status state machine | pi.dev's `AssistantMessageComponent` tracks lifecycle (streaming → complete / aborted / error). Enables accurate UI state, prevents stale rendering. |
| Spinner during LLM response | pi.dev's `Loader` provides continuous visual feedback during agent work. Prevents user uncertainty when LLM is thinking or tools are executing. |
| Context usage gauge in StatusBar | pi.dev's `FooterComponent` shows `context: N/M (X%)`. Critical feedback for multi-turn sessions approaching context window limits. |
| Editor with Emacs keybindings | pi.dev's `Editor` provides full multi-line editing experience. Single-line input is insufficient for complex prompts with code blocks or long messages. |
| Global keybinding registry | pi.dev's `KeybindingsManager` allows user customization and conflict detection. Hardcoded keybindings are not maintainable for a complex TUI. |
| Bracketed paste mode | pi.dev handles large pastes via `Event::Paste` and fold markers. Without this, pasting multi-line code floods the input buffer and causes frame drops. |
| Autocomplete with fd fallback | pi.dev's `CombinedAutocompleteProvider` supports slash commands and file paths. File search uses `fd` for speed but falls back to `std::fs` when unavailable. |

---

## 18. Out of Scope (MVP)

- Image rendering (Kitty/iTerm2 protocol)
- WASM/plugin-based extensions in the TUI
- Multi-tenant management UI (this is a client, not admin panel)
- Session persistence on the client side (server handles this)
- Streaming tool output (displaying tool results as they stream in)
- IME composition window support
- Terminal multiplexer (tmux) resize detection beyond SIGWINCH
- Session transcript export (JSON file export)
