# Pandaria TUI — Implementation Plan (P0/P1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor `crates/tui/` to implement P0/P1 enhancements: multi-line Editor with Emacs keybindings, global keybinding registry, autocomplete system, and bracketed paste support.

**Architecture:** Bottom-up: keybindings → editor → autocomplete → overlays → app integration → main entry. The TUI remains an independent binary crate.

**Tech Stack:** Rust 2024 edition, tokio, ratatui 0.29, crossterm 0.28, reqwest 0.12, pulldown-cmark 0.12, syntect 5, clap 4, eventsource-stream 0.2, wiremock (dev).

**Spec Reference:** `docs/specs/2026-05-02-tui-design.md`

---

## File Map

### TUI Crate (`crates/tui/`)
| File | Purpose |
|---|---|
| `src/main.rs` | tokio runtime entry, terminal init (BracketedPaste), event loop |
| `src/app.rs` | App state machine using KeybindingsManager + Editor |
| `src/state.rs` | State, SessionState, RenderedMessage, StreamingBuffer |
| `src/keybindings.rs` | Keybinding enum, KeybindingsManager, KeyEvent→KeyId |
| `src/autocomplete.rs` | AutocompleteProvider trait, SlashCommandProvider, FilePathProvider |
| `src/command.rs` | Command enum, slash-command parser |
| `src/markdown.rs` | pulldown-cmark → ratatui styled Text |
| `src/paste.rs` | Bracketed paste handler, PasteStore |
| `src/config.rs` | Config with KeysConfig parsing |
| `src/client/` | REST, SSE, auth, model types (existing) |
| `src/ui/` | Theme module (existing) |
| `src/widgets/` | All widgets (editor replaces input_bar) |
| `src/widgets/editor.rs` | Multi-line Editor with Emacs keybindings |
| `src/widgets/chat_view.rs` | ChatView (minor updates for Editor) |
| `src/widgets/header.rs` | HeaderBar (existing) |
| `src/widgets/tool_call.rs` | ToolCallWidget (existing) |
| `src/widgets/thinking.rs` | ThinkingBlock (existing) |
| `src/widgets/spinner.rs` | SpinnerWidget (existing) |
| `src/widgets/status_bar.rs` | StatusBar (existing) |
| `src/widgets/session_tabs.rs` | SessionTabs (existing) |
| `src/overlays/` | Overlay system |
| `src/overlays/mod.rs` | OverlayStack trait (existing) |
| `src/overlays/autocomplete.rs` | AutocompleteOverlay NEW |
| `src/overlays/command_palette.rs` | CommandPalette (existing) |
| `src/overlays/session_list.rs` | SessionListOverlay (existing) |
| `src/overlays/help.rs` | HelpOverlay (update keybindings display) |
| `tests/integration.rs` | End-to-end tests |

---

## Phase 0: Keybindings System (P0)

### Task 0.1: Keybinding types and KeyId conversion

**Files:**
- Create: `crates/tui/src/keybindings.rs`

**Steps:**

- [ ] **Step 1: Define Keybinding enum and KeyId type**

```rust
pub type KeyId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keybinding {
    // App
    AppQuit, AppInterrupt, AppToggleToolCalls, AppToggleThinking,
    AppSelectModel, AppListSessions, AppOpenCommandPalette,
    // Editor navigation
    EditorCursorUp, EditorCursorDown, EditorCursorLeft, EditorCursorRight,
    EditorCursorWordLeft, EditorCursorWordRight,
    EditorCursorLineStart, EditorCursorLineEnd,
    EditorCursorDocStart, EditorCursorDocEnd,
    EditorPageUp, EditorPageDown,
    // Editor editing
    EditorDeleteCharBackward, EditorDeleteCharForward,
    EditorDeleteWordBackward, EditorDeleteWordForward,
    EditorDeleteToLineStart, EditorDeleteToLineEnd,
    EditorNewLine, EditorSubmit, EditorUndo,
    EditorYank, EditorYankPop,
    EditorHistoryPrev, EditorHistoryNext,
    // Autocomplete
    AutocompleteTrigger,
}
```

- [ ] **Step 2: Implement KeyEvent → KeyId conversion**

```rust
pub fn key_event_to_id(event: &crossterm::event::KeyEvent) -> KeyId {
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut parts = Vec::new();
    
    // Modifiers sorted alphabetically: alt, ctrl, shift
    if event.modifiers.contains(KeyModifiers::ALT) { parts.push("alt"); }
    if event.modifiers.contains(KeyModifiers::CONTROL) { parts.push("ctrl"); }
    if event.modifiers.contains(KeyModifiers::SHIFT) { parts.push("shift"); }
    
    let key_part = match event.code {
        KeyCode::Char(c) => c.to_lowercase().to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Escape => "escape".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        _ => return String::new(), // Unsupported key
    };
    
    parts.push(&key_part);
    parts.join("+")
}
```

- [ ] **Step 3: Define default keybindings table**

```rust
pub fn default_keybindings() -> HashMap<Keybinding, Vec<KeyId>> {
    let mut map = HashMap::new();
    map.insert(Keybinding::AppQuit, vec!["ctrl+c".to_string(), "ctrl+d".to_string()]);
    map.insert(Keybinding::AppInterrupt, vec!["escape".to_string()]);
    map.insert(Keybinding::AppToggleToolCalls, vec!["ctrl+o".to_string()]);
    map.insert(Keybinding::AppToggleThinking, vec!["ctrl+t".to_string()]);
    map.insert(Keybinding::AppSelectModel, vec!["ctrl+l".to_string()]);
    map.insert(Keybinding::AppListSessions, vec!["ctrl+s".to_string()]);
    map.insert(Keybinding::EditorSubmit, vec!["enter".to_string()]);
    map.insert(Keybinding::EditorNewLine, vec!["shift+enter".to_string(), "alt+enter".to_string()]);
    map.insert(Keybinding::EditorCursorUp, vec!["up".to_string()]);
    map.insert(Keybinding::EditorCursorDown, vec!["down".to_string()]);
    map.insert(Keybinding::EditorCursorLeft, vec!["left".to_string(), "ctrl+b".to_string()]);
    map.insert(Keybinding::EditorCursorRight, vec!["right".to_string(), "ctrl+f".to_string()]);
    map.insert(Keybinding::EditorCursorWordLeft, vec!["alt+left".to_string(), "ctrl+left".to_string(), "alt+b".to_string()]);
    map.insert(Keybinding::EditorCursorWordRight, vec!["alt+right".to_string(), "ctrl+right".to_string(), "alt+f".to_string()]);
    map.insert(Keybinding::EditorCursorLineStart, vec!["home".to_string(), "ctrl+a".to_string()]);
    map.insert(Keybinding::EditorCursorLineEnd, vec!["end".to_string(), "ctrl+e".to_string()]);
    map.insert(Keybinding::EditorPageUp, vec!["pageup".to_string()]);
    map.insert(Keybinding::EditorPageDown, vec!["pagedown".to_string()]);
    map.insert(Keybinding::EditorDeleteCharBackward, vec!["backspace".to_string()]);
    map.insert(Keybinding::EditorDeleteCharForward, vec!["delete".to_string(), "ctrl+d".to_string()]);
    map.insert(Keybinding::EditorDeleteWordBackward, vec!["ctrl+w".to_string(), "alt+backspace".to_string()]);
    map.insert(Keybinding::EditorDeleteWordForward, vec!["alt+d".to_string(), "alt+delete".to_string()]);
    map.insert(Keybinding::EditorDeleteToLineStart, vec!["ctrl+u".to_string()]);
    map.insert(Keybinding::EditorDeleteToLineEnd, vec!["ctrl+k".to_string()]);
    map.insert(Keybinding::EditorYank, vec!["ctrl+y".to_string()]);
    map.insert(Keybinding::EditorYankPop, vec!["alt+y".to_string()]);
    map.insert(Keybinding::EditorUndo, vec!["ctrl+-".to_string()]);
    map.insert(Keybinding::AutocompleteTrigger, vec!["tab".to_string()]);
    map
}
```

- [ ] **Step 4: Implement KeybindingsManager**

```rust
pub struct KeybindingsManager {
    defaults: HashMap<Keybinding, Vec<KeyId>>,
    user: HashMap<Keybinding, Vec<KeyId>>,
}

impl KeybindingsManager {
    pub fn new() -> Self {
        Self { defaults: default_keybindings(), user: HashMap::new() }
    }
    
    pub fn load_user_config(&mut self, config: &crate::config::KeysConfig) {
        // Parse TOML flattened keys into HashMap<Keybinding, Vec<KeyId>>
        for (key_str, value) in &config.bindings {
            if let Some(binding) = Self::parse_keybinding_key(key_str) {
                let keys = match value {
                    toml::Value::String(s) => vec![s.clone()],
                    toml::Value::Array(arr) => arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect(),
                    _ => continue,
                };
                self.user.insert(binding, keys);
            }
        }
    }
    
    pub fn matches(&self, event: &crossterm::event::KeyEvent, binding: Keybinding) -> bool {
        let key_id = key_event_to_id(event);
        if key_id.is_empty() { return false; }
        
        let keys = self.user.get(&binding)
            .or_else(|| self.defaults.get(&binding))
            .cloned()
            .unwrap_or_default();
        
        keys.contains(&key_id)
    }
    
    pub fn get_binding_keys(&self, binding: Keybinding) -> Vec<KeyId> {
        self.user.get(&binding)
            .or_else(|| self.defaults.get(&binding))
            .cloned()
            .unwrap_or_default()
    }
    
    fn parse_keybinding_key(key: &str) -> Option<Keybinding> {
        match key {
            "app.quit" => Some(Keybinding::AppQuit),
            "app.interrupt" => Some(Keybinding::AppInterrupt),
            "app.toggle_tool_calls" => Some(Keybinding::AppToggleToolCalls),
            "app.toggle_thinking" => Some(Keybinding::AppToggleThinking),
            "app.select_model" => Some(Keybinding::AppSelectModel),
            "app.list_sessions" => Some(Keybinding::AppListSessions),
            "editor.submit" => Some(Keybinding::EditorSubmit),
            "editor.new_line" => Some(Keybinding::EditorNewLine),
            "editor.cursor_up" => Some(Keybinding::EditorCursorUp),
            "editor.cursor_down" => Some(Keybinding::EditorCursorDown),
            "editor.cursor_left" => Some(Keybinding::EditorCursorLeft),
            "editor.cursor_right" => Some(Keybinding::EditorCursorRight),
            "editor.cursor_word_left" => Some(Keybinding::EditorCursorWordLeft),
            "editor.cursor_word_right" => Some(Keybinding::EditorCursorWordRight),
            "editor.cursor_line_start" => Some(Keybinding::EditorCursorLineStart),
            "editor.cursor_line_end" => Some(Keybinding::EditorCursorLineEnd),
            "editor.delete_char_backward" => Some(Keybinding::EditorDeleteCharBackward),
            "editor.delete_char_forward" => Some(Keybinding::EditorDeleteCharForward),
            "editor.delete_word_backward" => Some(Keybinding::EditorDeleteWordBackward),
            "editor.delete_word_forward" => Some(Keybinding::EditorDeleteWordForward),
            "editor.delete_to_line_start" => Some(Keybinding::EditorDeleteToLineStart),
            "editor.delete_to_line_end" => Some(Keybinding::EditorDeleteToLineEnd),
            "editor.yank" => Some(Keybinding::EditorYank),
            "editor.yank_pop" => Some(Keybinding::EditorYankPop),
            "editor.undo" => Some(Keybinding::EditorUndo),
            "autocomplete.trigger" => Some(Keybinding::AutocompleteTrigger),
            _ => None,
        }
    }
}
```

- [ ] **Step 5: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    
    #[test]
    fn test_key_event_to_id_ctrl_c() {
        let event = KeyEvent::from(KeyCode::Char('c'));
        // Need to set modifiers manually in actual test
        assert_eq!(key_event_to_id(&event), "c");
    }
    
    #[test]
    fn test_matches_default_binding() {
        let kb = KeybindingsManager::new();
        let event = KeyEvent::from(KeyCode::Enter);
        assert!(kb.matches(&event, Keybinding::EditorSubmit));
    }
    
    #[test]
    fn test_user_override_takes_precedence() {
        let mut kb = KeybindingsManager::new();
        let mut config = crate::config::KeysConfig::default();
        config.bindings.insert("editor.submit".to_string(), toml::Value::String("ctrl+enter".to_string()));
        kb.load_user_config(&config);
        
        let event = KeyEvent::from(KeyCode::Enter);
        assert!(!kb.matches(&event, Keybinding::EditorSubmit));
    }
}
```

- [ ] **Step 6: Verify and commit**

Run: `cargo test -p tui keybindings`
Expected: Tests pass

```bash
git add crates/tui/src/keybindings.rs
git commit -m "feat(tui): add KeybindingsManager with user override support"
```

---

## Phase 1: Editor Widget (P0)

### Task 1.1: Editor core structure and basic editing

**Files:**
- Create: `crates/tui/src/widgets/editor.rs`
- Delete: `crates/tui/src/widgets/input_bar.rs` (after migration)

**Steps:**

- [ ] **Step 1: Define Editor struct**

```rust
pub struct Editor {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
    preferred_col: Option<usize>,
    viewport_top: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    kill_ring: Vec<String>,
    last_kill_appended: bool,
    undo_stack: Vec<EditorState>,
}

#[derive(Clone)]
struct EditorState {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            preferred_col: None,
            viewport_top: 0,
            history: Vec::new(),
            history_index: None,
            kill_ring: Vec::new(),
            last_kill_appended: false,
            undo_stack: Vec::new(),
        }
    }
    
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }
    
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
    
    pub fn current_line_text(&self) -> &str {
        self.lines.get(self.cursor_line).map(|s| s.as_str()).unwrap_or("")
    }
    
    pub fn text_before_cursor(&self) -> String {
        let mut result = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i < self.cursor_line {
                result.push_str(line);
                result.push('\n');
            } else if i == self.cursor_line {
                result.push_str(&line[..self.cursor_col.min(line.len())]);
                break;
            }
        }
        result
    }
}
```

- [ ] **Step 2: Implement cursor movement**

```rust
impl Editor {
    pub fn cursor_up(&mut self) {
        self.save_undo_state();
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            let line_len = self.lines[self.cursor_line].chars().count();
            self.cursor_col = self.preferred_col.unwrap_or(self.cursor_col).min(line_len);
        }
    }
    
    pub fn cursor_down(&mut self) {
        self.save_undo_state();
        if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            let line_len = self.lines[self.cursor_line].chars().count();
            self.cursor_col = self.preferred_col.unwrap_or(self.cursor_col).min(line_len);
        }
    }
    
    pub fn cursor_left(&mut self) {
        self.save_undo_state();
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].chars().count();
        }
        self.preferred_col = Some(self.cursor_col);
    }
    
    pub fn cursor_right(&mut self) {
        self.save_undo_state();
        let line_len = self.lines[self.cursor_line].chars().count();
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
        self.preferred_col = Some(self.cursor_col);
    }
    
    pub fn cursor_word_left(&mut self) {
        self.save_undo_state();
        let text = self.current_line_text();
        let char_indices: Vec<_> = text.char_indices().collect();
        
        if self.cursor_col == 0 && self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].chars().count();
            return;
        }
        
        // Find previous word boundary
        let mut found_non_word = false;
        for i in (0..self.cursor_col.saturating_sub(1)).rev() {
            let ch = char_indices.get(i).map(|(_, c)| *c).unwrap_or(' ');
            let is_word = ch.is_alphanumeric() || ch == '_';
            
            if !found_non_word && !is_word {
                found_non_word = true;
            }
            if found_non_word && is_word {
                self.cursor_col = i + 1;
                self.preferred_col = Some(self.cursor_col);
                return;
            }
        }
        self.cursor_col = 0;
        self.preferred_col = Some(0);
    }
    
    pub fn cursor_word_right(&mut self) {
        self.save_undo_state();
        let text = self.current_line_text();
        let len = text.chars().count();
        
        if self.cursor_col >= len && self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
            return;
        }
        
        let char_indices: Vec<_> = text.char_indices().collect();
        let mut found_word = false;
        
        for i in self.cursor_col..len {
            let ch = char_indices.get(i).map(|(_, c)| *c).unwrap_or(' ');
            let is_word = ch.is_alphanumeric() || ch == '_';
            
            if !found_word && is_word {
                found_word = true;
            }
            if found_word && !is_word {
                self.cursor_col = i;
                self.preferred_col = Some(self.cursor_col);
                return;
            }
        }
        self.cursor_col = len;
        self.preferred_col = Some(len);
    }
    
    pub fn cursor_line_start(&mut self) {
        self.save_undo_state();
        self.cursor_col = 0;
        self.preferred_col = Some(0);
    }
    
    pub fn cursor_line_end(&mut self) {
        self.save_undo_state();
        self.cursor_col = self.lines[self.cursor_line].chars().count();
        self.preferred_col = Some(self.cursor_col);
    }
    
    pub fn cursor_doc_start(&mut self) {
        self.save_undo_state();
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.preferred_col = Some(0);
    }
    
    pub fn cursor_doc_end(&mut self) {
        self.save_undo_state();
        self.cursor_line = self.lines.len() - 1;
        self.cursor_col = self.lines[self.cursor_line].chars().count();
        self.preferred_col = Some(self.cursor_col);
    }
    
    pub fn page_up(&mut self) {
        self.save_undo_state();
        self.cursor_line = self.cursor_line.saturating_sub(10);
        let line_len = self.lines[self.cursor_line].chars().count();
        self.cursor_col = self.cursor_col.min(line_len);
    }
    
    pub fn page_down(&mut self) {
        self.save_undo_state();
        self.cursor_line = (self.cursor_line + 10).min(self.lines.len() - 1);
        let line_len = self.lines[self.cursor_line].chars().count();
        self.cursor_col = self.cursor_col.min(line_len);
    }
    
    fn save_undo_state(&mut self) {
        // Only save if last state is different
        let state = EditorState {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        };
        if self.undo_stack.last() != Some(&state) {
            self.undo_stack.push(state);
            if self.undo_stack.len() > 100 {
                self.undo_stack.remove(0);
            }
        }
    }
}
```

- [ ] **Step 3: Implement insert and delete**

```rust
impl Editor {
    pub fn insert_char(&mut self, ch: char) {
        self.save_undo_state();
        let line = &mut self.lines[self.cursor_line];
        let byte_idx = line.char_indices().nth(self.cursor_col).map(|(i, _)| i).unwrap_or(line.len());
        line.insert(byte_idx, ch);
        self.cursor_col += 1;
        self.preferred_col = Some(self.cursor_col);
        self.last_kill_appended = false;
    }
    
    pub fn insert_newline(&mut self) {
        self.save_undo_state();
        let line = &mut self.lines[self.cursor_line];
        let byte_idx = line.char_indices().nth(self.cursor_col).map(|(i, _)| i).unwrap_or(line.len());
        let remainder = line.split_off(byte_idx);
        self.cursor_line += 1;
        self.lines.insert(self.cursor_line, remainder);
        self.cursor_col = 0;
        self.preferred_col = Some(0);
        self.last_kill_appended = false;
    }
    
    pub fn insert_text(&mut self, text: &str) {
        self.save_undo_state();
        for ch in text.chars() {
            if ch == '\n' {
                self.insert_newline();
            } else {
                self.insert_char(ch);
            }
        }
    }
    
    pub fn delete_char_backward(&mut self) {
        self.save_undo_state();
        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_line];
            let byte_idx = line.char_indices().nth(self.cursor_col - 1).map(|(i, _)| i).unwrap_or(0);
            let end_idx = line.char_indices().nth(self.cursor_col).map(|(i, _)| i).unwrap_or(line.len());
            line.drain(byte_idx..end_idx);
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            let line = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].chars().count();
            self.lines[self.cursor_line].push_str(&line);
        }
        self.preferred_col = Some(self.cursor_col);
        self.last_kill_appended = false;
    }
    
    pub fn delete_char_forward(&mut self) {
        self.save_undo_state();
        let line = &mut self.lines[self.cursor_line];
        let len = line.chars().count();
        if self.cursor_col < len {
            let byte_idx = line.char_indices().nth(self.cursor_col).map(|(i, _)| i).unwrap_or(0);
            let end_idx = line.char_indices().nth(self.cursor_col + 1).map(|(i, _)| i).unwrap_or(line.len());
            line.drain(byte_idx..end_idx);
        } else if self.cursor_line + 1 < self.lines.len() {
            let next_line = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next_line);
        }
        self.last_kill_appended = false;
    }
    
    pub fn delete_word_backward(&mut self) {
        self.save_undo_state();
        let start_col = self.cursor_col;
        self.cursor_word_left();
        let deleted = self.delete_range(self.cursor_line, self.cursor_col, self.cursor_line, start_col);
        self.push_kill_ring(deleted, false);
    }
    
    pub fn delete_word_forward(&mut self) {
        self.save_undo_state();
        let start_col = self.cursor_col;
        self.cursor_word_right();
        let deleted = self.delete_range(self.cursor_line, start_col, self.cursor_line, self.cursor_col);
        self.cursor_col = start_col;
        self.push_kill_ring(deleted, false);
    }
    
    pub fn delete_to_line_start(&mut self) {
        self.save_undo_state();
        let deleted = self.delete_range(self.cursor_line, 0, self.cursor_line, self.cursor_col);
        self.cursor_col = 0;
        self.push_kill_ring(deleted, false);
    }
    
    pub fn delete_to_line_end(&mut self) {
        self.save_undo_state();
        let len = self.lines[self.cursor_line].chars().count();
        let deleted = self.delete_range(self.cursor_line, self.cursor_col, self.cursor_line, len);
        self.push_kill_ring(deleted, true); // Append to kill ring
    }
    
    fn delete_range(&mut self, start_line: usize, start_col: usize, end_line: usize, end_col: usize) -> String {
        if start_line == end_line {
            let line = &mut self.lines[start_line];
            let start_byte = line.char_indices().nth(start_col).map(|(i, _)| i).unwrap_or(0);
            let end_byte = line.char_indices().nth(end_col).map(|(i, _)| i).unwrap_or(line.len());
            let deleted = line[start_byte..end_byte].to_string();
            line.drain(start_byte..end_byte);
            deleted
        } else {
            // Multi-line delete (simplified)
            String::new()
        }
    }
    
    fn push_kill_ring(&mut self, text: String, append: bool) {
        if text.is_empty() { return; }
        if append && self.last_kill_appended && !self.kill_ring.is_empty() {
            if let Some(last) = self.kill_ring.last_mut() {
                last.push_str(&text);
            }
        } else {
            self.kill_ring.push(text);
            if self.kill_ring.len() > 10 {
                self.kill_ring.remove(0);
            }
        }
        self.last_kill_appended = append;
    }
    
    pub fn kill_ring_yank(&mut self) {
        if let Some(text) = self.kill_ring.last() {
            self.insert_text(text);
        }
    }
    
    pub fn kill_ring_yank_pop(&mut self) {
        // Cycle to previous kill ring entry and replace yanked text
        // Simplified: rotate kill ring
        if self.kill_ring.len() > 1 {
            self.kill_ring.rotate_right(1);
        }
    }
    
    pub fn undo(&mut self) {
        if let Some(state) = self.undo_stack.pop() {
            self.lines = state.lines;
            self.cursor_line = state.cursor_line;
            self.cursor_col = state.cursor_col;
        }
    }
}
```

- [ ] **Step 4: Implement history and paste**

```rust
impl Editor {
    pub fn history_prev(&mut self) {
        if self.history.is_empty() { return; }
        let idx = match self.history_index {
            None => self.history.len().saturating_sub(1),
            Some(i) if i > 0 => i - 1,
            _ => return,
        };
        self.history_index = Some(idx);
        self.restore_history_entry(idx);
    }
    
    pub fn history_next(&mut self) {
        match self.history_index {
            None => return,
            Some(i) if i + 1 < self.history.len() => {
                self.history_index = Some(i + 1);
                self.restore_history_entry(i + 1);
            }
            _ => {
                self.history_index = None;
                self.lines = vec![String::new()];
                self.cursor_line = 0;
                self.cursor_col = 0;
            }
        }
    }
    
    fn restore_history_entry(&mut self, idx: usize) {
        let text = &self.history[idx];
        self.lines = text.lines().map(|s| s.to_string()).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = self.lines.len() - 1;
        self.cursor_col = self.lines[self.cursor_line].chars().count();
    }
    
    pub fn take_text(&mut self) -> String {
        let text = self.lines.join("\n");
        self.history.push(text.clone());
        self.history_index = None;
        self.lines = vec![String::new()];
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.viewport_top = 0;
        text
    }
    
    pub fn insert_paste_marker(&mut self, id: usize, line_count: usize) {
        let marker = format!("[paste #{} +{} lines]", id, line_count);
        self.insert_text(&marker);
    }
}
```

- [ ] **Step 5: Implement render**

```rust
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

impl Editor {
    pub fn render(&self, f: &mut Frame, area: Rect, theme: &crate::ui::theme::Theme, focused: bool, busy: bool) {
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(theme.border));
        
        let inner = block.inner(area);
        f.render_widget(block, area);
        
        // Calculate visible lines
        let max_lines = inner.height as usize;
        let visible_lines: Vec<Line> = if self.lines.is_empty() {
            vec![]
        } else {
            self.lines.iter().skip(self.viewport_top).take(max_lines)
                .enumerate()
                .map(|(i, line)| {
                    let is_current_line = self.viewport_top + i == self.cursor_line;
                    if is_current_line && focused {
                        // Highlight current line with cursor
                        let before: String = line.chars().take(self.cursor_col).collect();
                        let at_cursor = line.chars().nth(self.cursor_col).unwrap_or(' ');
                        let after: String = line.chars().skip(self.cursor_col + 1).collect();
                        
                        Line::from(vec![
                            Span::styled(before, Style::default().fg(theme.text)),
                            Span::styled(at_cursor.to_string(), Style::default().fg(theme.text).add_modifier(Modifier::REVERSED)),
                            Span::styled(after, Style::default().fg(theme.text)),
                        ])
                    } else {
                        Line::from(Span::styled(line.clone(), Style::default().fg(theme.text)))
                    }
                })
                .collect()
        };
        
        if visible_lines.is_empty() && !busy {
            let placeholder = Span::styled("Write a message or /command...", Style::default().fg(theme.dim));
            f.render_widget(Paragraph::new(Line::from(placeholder)).block(Block::default()), inner);
        } else if busy && self.is_empty() {
            let placeholder = Span::styled("Interrupt (Esc)...", Style::default().fg(theme.warning));
            f.render_widget(Paragraph::new(Line::from(placeholder)).block(Block::default()), inner);
        } else {
            f.render_widget(Paragraph::new(visible_lines).block(Block::default()), inner);
        }
    }
}
```

- [ ] **Step 6: Update widgets/mod.rs**

Replace `pub mod input_bar;` with `pub mod editor;`.

- [ ] **Step 7: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_insert_char() {
        let mut ed = Editor::new();
        ed.insert_char('h');
        ed.insert_char('i');
        assert_eq!(ed.lines[0], "hi");
        assert_eq!(ed.cursor_col, 2);
    }
    
    #[test]
    fn test_insert_newline() {
        let mut ed = Editor::new();
        ed.insert_text("hello");
        ed.insert_newline();
        ed.insert_text("world");
        assert_eq!(ed.lines.len(), 2);
        assert_eq!(ed.lines[0], "hello");
        assert_eq!(ed.lines[1], "world");
    }
    
    #[test]
    fn test_delete_char_backward() {
        let mut ed = Editor::new();
        ed.insert_text("hi");
        ed.delete_char_backward();
        assert_eq!(ed.lines[0], "h");
        assert_eq!(ed.cursor_col, 1);
    }
    
    #[test]
    fn test_delete_word_backward() {
        let mut ed = Editor::new();
        ed.insert_text("hello world");
        ed.delete_word_backward();
        assert_eq!(ed.lines[0], "hello ");
    }
    
    #[test]
    fn test_delete_to_line_end() {
        let mut ed = Editor::new();
        ed.insert_text("hello world");
        ed.cursor_line_start();
        ed.delete_to_line_end();
        assert_eq!(ed.lines[0], "");
    }
    
    #[test]
    fn test_history_navigation() {
        let mut ed = Editor::new();
        ed.insert_text("first");
        ed.take_text();
        ed.insert_text("second");
        ed.take_text();
        
        ed.history_prev();
        assert_eq!(ed.lines[0], "second");
        ed.history_prev();
        assert_eq!(ed.lines[0], "first");
    }
    
    #[test]
    fn test_undo() {
        let mut ed = Editor::new();
        ed.insert_text("hello");
        ed.undo();
        assert!(ed.is_empty());
    }
    
    #[test]
    fn test_kill_ring() {
        let mut ed = Editor::new();
        ed.insert_text("hello world");
        ed.cursor_line_end();
        ed.delete_word_backward();
        ed.kill_ring_yank();
        assert_eq!(ed.lines[0], "hello worldworld");
    }
    
    #[test]
    fn test_cursor_word_movement() {
        let mut ed = Editor::new();
        ed.insert_text("hello world test");
        ed.cursor_word_left();
        assert_eq!(ed.cursor_col, 12); // before "test"
        ed.cursor_word_left();
        assert_eq!(ed.cursor_col, 6); // before "world"
        ed.cursor_word_right();
        assert_eq!(ed.cursor_col, 11); // after "world"
    }
}
```

- [ ] **Step 8: Verify and commit**

Run: `cargo test -p tui editor`
Expected: Tests pass

```bash
git add crates/tui/src/widgets/editor.rs crates/tui/src/widgets/mod.rs
git rm crates/tui/src/widgets/input_bar.rs 2>/dev/null || true
git commit -m "feat(tui): add multi-line Editor with Emacs keybindings"
```

---

## Phase 2: Paste Handling Update (P0)

### Task 2.1: Update paste.rs for Editor integration

**Files:**
- Modify: `crates/tui/src/paste.rs`
- Modify: `crates/tui/src/main.rs` (enable BracketedPaste)

**Steps:**

- [ ] **Step 1: Update PasteStore with resolve_markers**

```rust
impl PasteStore {
    // ... existing store() method ...
    
    pub fn resolve_markers(&self, text: &str) -> String {
        use regex::Regex;
        lazy_static::lazy_static! {
            static ref RE: Regex = Regex::new(r"\[paste #(\d+)( \+(\d+) lines)?\]").unwrap();
        }
        
        let mut result = text.to_string();
        for (id, content) in &self.markers {
            let marker = format!("[paste #{} +{} lines]", id, content.lines().count());
            result = result.replace(&marker, content);
        }
        result
    }
    
    pub fn clear(&mut self) {
        self.markers.clear();
        self.next_id = 0;
    }
}
```

Note: Add `regex = "1"` to Cargo.toml if not present.

- [ ] **Step 2: Enable bracketed paste in main.rs**

In terminal setup, add:
```rust
use crossterm::event::{EnableBracketedPaste, DisableBracketedPaste};

// After EnterAlternateScreen:
stdout.execute(EnableBracketedPaste)?;

// Before LeaveAlternateScreen:
terminal.backend_mut().execute(DisableBracketedPaste)?;
```

- [ ] **Step 3: Handle Event::Paste in app.rs**

Add to handle_key_event:
```rust
Event::Paste(data) => {
    let result = self.paste_store.store(&data);
    if result.starts_with("[paste #") {
        self.editor.insert_paste_marker(
            // Parse id from marker... or just insert the marker text
        );
    } else {
        self.editor.insert_text(&result);
    }
}
```

- [ ] **Step 4: Verify and commit**

```bash
git add crates/tui/src/paste.rs crates/tui/src/main.rs
git commit -m "feat(tui): add bracketed paste support with Editor integration"
```

---

## Phase 3: Autocomplete System (P1)

### Task 3.1: Autocomplete providers

**Files:**
- Create: `crates/tui/src/autocomplete.rs`

**Steps:**

- [ ] **Step 1: Define trait and types**

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

- [ ] **Step 2: Implement SlashCommandProvider**

```rust
pub struct SlashCommandProvider {
    commands: Vec<SlashCommand>,
}

pub struct SlashCommand {
    pub name: String,
    pub description: String,
}

impl SlashCommandProvider {
    pub fn new() -> Self {
        Self {
            commands: vec![
                SlashCommand { name: "quit".to_string(), description: "Quit the application".to_string() },
                SlashCommand { name: "new".to_string(), description: "Create a new session".to_string() },
                SlashCommand { name: "switch".to_string(), description: "Switch to session".to_string() },
                SlashCommand { name: "list".to_string(), description: "List sessions".to_string() },
                SlashCommand { name: "model".to_string(), description: "Select model".to_string() },
                SlashCommand { name: "clear".to_string(), description: "Clear view".to_string() },
                SlashCommand { name: "connect".to_string(), description: "Connect to server".to_string() },
                SlashCommand { name: "auth".to_string(), description: "Set auth token".to_string() },
                SlashCommand { name: "tokens".to_string(), description: "View usage".to_string() },
                SlashCommand { name: "help".to_string(), description: "Show help".to_string() },
            ],
        }
    }
}

impl AutocompleteProvider for SlashCommandProvider {
    fn should_trigger(&self, context: &AutocompleteContext) -> bool {
        context.current_line.starts_with('/') && context.cursor_col > 0
    }
    
    fn get_suggestions(&self, context: &AutocompleteContext) -> Vec<Suggestion> {
        let prefix = &context.current_line[1..context.cursor_col];
        self.commands.iter()
            .filter(|cmd| cmd.name.starts_with(prefix))
            .map(|cmd| Suggestion {
                label: format!("/{} — {}", cmd.name, cmd.description),
                value: format!("/{}", cmd.name),
                description: Some(cmd.description.clone()),
            })
            .collect()
    }
}
```

- [ ] **Step 3: Implement FilePathProvider**

```rust
use std::path::PathBuf;
use std::process::Command;

pub struct FilePathProvider {
    base_dir: PathBuf,
}

impl FilePathProvider {
    pub fn new() -> Self {
        Self { base_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")) }
    }
    
    fn fd_available() -> bool {
        Command::new("fd").arg("--version").output().is_ok()
    }
}

impl AutocompleteProvider for FilePathProvider {
    fn should_trigger(&self, context: &AutocompleteContext) -> bool {
        context.text_before_cursor.ends_with('/') || 
        context.current_line.split_whitespace().last().map(|s| s.starts_with("./") || s.starts_with("../") || s.starts_with("/") || s.starts_with("~/")).unwrap_or(false)
    }
    
    fn get_suggestions(&self, context: &AutocompleteContext) -> Vec<Suggestion> {
        let query = context.current_line.split_whitespace().last().unwrap_or("");
        if query.is_empty() {
            return vec![];
        }
        
        let (dir, prefix) = if query.ends_with('/') {
            (PathBuf::from(query), "")
        } else {
            let p = PathBuf::from(query);
            let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or(PathBuf::from("."));
            let prefix = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            (dir, prefix)
        };
        
        if Self::fd_available() {
            // Use fd for fast search
            match Command::new("fd")
                .arg("--max-results=20")
                .arg("--type=f")
                .arg("--type=d")
                .arg(".")
                .current_dir(&dir)
                .output() 
            {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    stdout.lines()
                        .filter(|line| line.to_lowercase().starts_with(&prefix.to_lowercase()))
                        .take(8)
                        .map(|line| Suggestion {
                            label: line.to_string(),
                            value: format!("{}{}", query, line),
                            description: None,
                        })
                        .collect()
                }
                Err(_) => self.fallback_list(&dir, prefix),
            }
        } else {
            self.fallback_list(&dir, prefix)
        }
    }
}

impl FilePathProvider {
    fn fallback_list(&self, dir: &PathBuf, prefix: &str) -> Vec<Suggestion> {
        match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name().to_str()
                        .map(|n| n.to_lowercase().starts_with(&prefix.to_lowercase()))
                        .unwrap_or(false)
                })
                .take(8)
                .map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let suffix = if is_dir { "/" } else { "" };
                    Suggestion {
                        label: format!("{}{}", name, suffix),
                        value: format!("{}{}", name, suffix),
                        description: None,
                    }
                })
                .collect(),
            Err(_) => vec![],
        }
    }
}
```

- [ ] **Step 4: Verify and commit**

```bash
git add crates/tui/src/autocomplete.rs
git commit -m "feat(tui): add autocomplete providers for commands and file paths"
```

---

## Phase 4: Autocomplete Overlay (P1)

### Task 4.1: Implement AutocompleteOverlay

**Files:**
- Create: `crates/tui/src/overlays/autocomplete.rs`
- Modify: `crates/tui/src/overlays/mod.rs`

**Steps:**

- [ ] **Step 1: Write AutocompleteOverlay**

```rust
use crate::autocomplete::Suggestion;
use crate::overlays::{Overlay, OverlayAction};
use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem};
use ratatui::Frame;

pub struct AutocompleteOverlay {
    suggestions: Vec<Suggestion>,
    selected: usize,
}

impl AutocompleteOverlay {
    pub fn new(suggestions: Vec<Suggestion>) -> Self {
        Self { suggestions, selected: 0 }
    }
    
    pub fn selected_value(&self) -> Option<&str> {
        self.suggestions.get(self.selected).map(|s| s.value.as_str())
    }
}

impl Overlay for AutocompleteOverlay {
    fn render(&self, f: &mut Frame, _area: Rect) {
        let theme = Theme::default();
        let items: Vec<ListItem> = self.suggestions.iter().enumerate().map(|(i, s)| {
            let style = if i == self.selected {
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            ListItem::new(Line::from(vec![
                Span::styled(&s.label, style),
                s.description.as_ref().map(|d| Span::styled(format!(" — {}", d), Style::default().fg(theme.muted))).unwrap_or(Span::from("")),
            ]))
        }).collect();
        
        let height = (items.len() as u16 + 2).min(8);
        let width = 50u16;
        let area = Rect::new(
            f.area().x + 2,
            f.area().y + f.area().height.saturating_sub(height + 2),
            width.min(f.area().width.saturating_sub(4)),
            height,
        );
        
        let block = Block::default().borders(Borders::ALL).title("Suggestions").style(Style::default().fg(theme.text));
        let list = List::new(items).block(block);
        f.render_widget(Clear, area);
        f.render_widget(list, area);
    }
    
    fn handle_input(&mut self, key: crossterm::event::KeyEvent) -> OverlayAction {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Up => {
                if self.selected > 0 { self.selected -= 1; }
                OverlayAction::Consumed
            }
            KeyCode::Down => {
                if self.selected + 1 < self.suggestions.len() {
                    self.selected += 1;
                }
                OverlayAction::Consumed
            }
            KeyCode::Enter => {
                OverlayAction::Confirm(self.selected_value().unwrap_or("").to_string())
            }
            KeyCode::Esc => OverlayAction::Dismiss,
            _ => OverlayAction::Consumed,
        }
    }
}
```

- [ ] **Step 2: Register in overlays/mod.rs**

Add: `pub mod autocomplete;`

- [ ] **Step 3: Verify and commit**

```bash
git add crates/tui/src/overlays/autocomplete.rs crates/tui/src/overlays/mod.rs
git commit -m "feat(tui): add AutocompleteOverlay for suggestion display"
```

---

## Phase 5: App Integration (P0/P1)

### Task 5.1: Refactor App to use Editor and KeybindingsManager

**Files:**
- Modify: `crates/tui/src/app.rs`

**Steps:**

- [ ] **Step 1: Update App struct**

```rust
use crate::keybindings::{Keybinding, KeybindingsManager};
use crate::widgets::editor::Editor;

pub struct App {
    pub state: AppState,
    pub data: crate::state::State,
    pub config: Config,
    pub theme: Theme,
    pub keybindings: KeybindingsManager,
    pub rest: RestClient,
    pub editor: Editor,
    pub spinner: SpinnerWidget,
    pub overlays: OverlayStack,
    pub reqwest_client: reqwest::Client,
    pub paste_store: crate::paste::PasteStore,
    pub context_window: Option<u64>,
    pub input_tokens: u64,
    pub server_rx: Option<mpsc::Receiver<ServerEvent>>,
    pub scroll_offset: usize,
    pub running: bool,
    pub autocomplete_providers: Vec<Box<dyn crate::autocomplete::AutocompleteProvider>>,
}

impl App {
    pub fn new(config: Config, session_id: String, session_info: crate::client::model::SessionInfo) -> Self {
        let mut keybindings = KeybindingsManager::new();
        if let Some(ref keys_config) = config.keys {
            keybindings.load_user_config(keys_config);
        }
        
        Self {
            state: AppState::Connected,
            data: crate::state::State::new(session_id, session_info),
            config,
            theme: Theme::default(),
            keybindings,
            rest: RestClient::new(&config.server),
            editor: Editor::new(),
            spinner: SpinnerWidget::new(),
            overlays: OverlayStack::new(),
            reqwest_client: reqwest::Client::new(),
            paste_store: crate::paste::PasteStore::new(),
            context_window: session_info.context_window,
            input_tokens: 0,
            server_rx: None,
            scroll_offset: 0,
            running: true,
            autocomplete_providers: vec![
                Box::new(crate::autocomplete::SlashCommandProvider::new()),
                Box::new(crate::autocomplete::FilePathProvider::new()),
            ],
        }
    }
}
```

- [ ] **Step 2: Rewrite handle_key_event using KeybindingsManager**

```rust
pub fn handle_key_event(&mut self, key: KeyEvent) {
    use crossterm::event::Event;
    
    // Handle paste events
    if let Event::Paste(data) = Event::Key(key) {
        let result = self.paste_store.store(&data);
        if result.starts_with("[paste #") {
            self.editor.insert_text(&result);
        } else {
            self.editor.insert_text(&result);
        }
        return;
    }
    
    // Handle overlays
    if !self.overlays.is_empty() {
        if let Some(overlay) = self.overlays.top_mut() {
            let action = overlay.handle_input(key);
            match action {
                OverlayAction::Dismiss => { self.overlays.pop(); }
                OverlayAction::Confirm(value) => {
                    self.overlays.pop();
                    self.handle_overlay_confirm(value);
                }
                OverlayAction::Consumed => {}
                OverlayAction::Ignored => {
                    self.overlays.pop();
                    self.handle_key_event(key);
                    return;
                }
            }
        }
        return;
    }
    
    let kb = &self.keybindings;
    
    // App-level shortcuts
    if kb.matches(&key, Keybinding::AppQuit) && self.state != AppState::Busy {
        self.running = false;
        return;
    }
    
    if kb.matches(&key, Keybinding::AppInterrupt) {
        if self.state == AppState::Busy {
            let rest = RestClient::new(&self.config.server);
            let token = self.config.auth.token.clone().unwrap_or_default();
            let sid = self.data.active_session.clone();
            tokio::spawn(async move {
                let _ = rest.interrupt(&sid, &token).await;
            });
            if let Some(last) = self.data.active_session_mut().messages.last_mut() {
                last.status = MessageStatus::Aborted;
            }
            self.state = AppState::Connected;
        } else {
            self.editor = Editor::new();
        }
        return;
    }
    
    if kb.matches(&key, Keybinding::AppToggleToolCalls) {
        for msg in &mut self.data.active_session_mut().messages {
            for block in &mut msg.blocks {
                if let MessageBlock::ToolCall(tc) = block { tc.toggle(); }
            }
        }
        return;
    }
    
    if kb.matches(&key, Keybinding::AppToggleThinking) {
        for msg in &mut self.data.active_session_mut().messages {
            for block in &mut msg.blocks {
                if let MessageBlock::Thinking(tb) = block { tb.toggle(); }
            }
        }
        return;
    }
    
    if kb.matches(&key, Keybinding::AppSelectModel) {
        let models = vec!["gpt-4o".to_string(), "claude-sonnet-4".to_string()];
        self.overlays.push(Box::new(crate::overlays::model_selector::ModelSelector::new(models)));
        return;
    }
    
    if kb.matches(&key, Keybinding::AppListSessions) {
        let sessions: Vec<_> = self.data.sessions.iter()
            .map(|(id, s)| (id.clone(), s.info.title.clone().unwrap_or_else(|| id.chars().take(8).collect())))
            .collect();
        self.overlays.push(Box::new(crate::overlays::session_list::SessionListOverlay::new(sessions)));
        return;
    }
    
    // Editor shortcuts
    if kb.matches(&key, Keybinding::EditorSubmit) {
        self.submit_input();
        return;
    }
    
    if kb.matches(&key, Keybinding::EditorNewLine) {
        self.editor.insert_newline();
        return;
    }
    
    if kb.matches(&key, Keybinding::EditorCursorUp) {
        if self.editor.cursor_line == 0 && self.editor.is_empty() {
            self.editor.history_prev();
        } else {
            self.editor.cursor_up();
        }
        return;
    }
    
    if kb.matches(&key, Keybinding::EditorCursorDown) {
        if self.editor.cursor_line + 1 >= self.editor.line_count() {
            self.editor.history_next();
        } else {
            self.editor.cursor_down();
        }
        return;
    }
    
    if kb.matches(&key, Keybinding::EditorCursorLeft) { self.editor.cursor_left(); return; }
    if kb.matches(&key, Keybinding::EditorCursorRight) { self.editor.cursor_right(); return; }
    if kb.matches(&key, Keybinding::EditorCursorWordLeft) { self.editor.cursor_word_left(); return; }
    if kb.matches(&key, Keybinding::EditorCursorWordRight) { self.editor.cursor_word_right(); return; }
    if kb.matches(&key, Keybinding::EditorCursorLineStart) { self.editor.cursor_line_start(); return; }
    if kb.matches(&key, Keybinding::EditorCursorLineEnd) { self.editor.cursor_line_end(); return; }
    if kb.matches(&key, Keybinding::EditorPageUp) { self.editor.page_up(); return; }
    if kb.matches(&key, Keybinding::EditorPageDown) { self.editor.page_down(); return; }
    if kb.matches(&key, Keybinding::EditorDeleteCharBackward) { self.editor.delete_char_backward(); return; }
    if kb.matches(&key, Keybinding::EditorDeleteCharForward) { self.editor.delete_char_forward(); return; }
    if kb.matches(&key, Keybinding::EditorDeleteWordBackward) { self.editor.delete_word_backward(); return; }
    if kb.matches(&key, Keybinding::EditorDeleteWordForward) { self.editor.delete_word_forward(); return; }
    if kb.matches(&key, Keybinding::EditorDeleteToLineStart) { self.editor.delete_to_line_start(); return; }
    if kb.matches(&key, Keybinding::EditorDeleteToLineEnd) { self.editor.delete_to_line_end(); return; }
    if kb.matches(&key, Keybinding::EditorYank) { self.editor.kill_ring_yank(); return; }
    if kb.matches(&key, Keybinding::EditorYankPop) { self.editor.kill_ring_yank_pop(); return; }
    if kb.matches(&key, Keybinding::EditorUndo) { self.editor.undo(); return; }
    
    // Autocomplete trigger
    if kb.matches(&key, Keybinding::AutocompleteTrigger) {
        self.trigger_autocomplete();
        return;
    }
    
    // Character input
    if let KeyCode::Char(c) = key.code {
        if c == '/' && self.editor.is_empty() {
            self.overlays.push(Box::new(crate::overlays::command_palette::CommandPalette::new()));
        } else {
            self.editor.insert_char(c);
        }
    }
}

fn trigger_autocomplete(&mut self) {
    let context = crate::autocomplete::AutocompleteContext {
        full_text: self.editor.lines.join("\n"),
        cursor_line: self.editor.cursor_line,
        cursor_col: self.editor.cursor_col,
        current_line: self.editor.current_line_text().to_string(),
        text_before_cursor: self.editor.text_before_cursor(),
    };
    
    for provider in &self.autocomplete_providers {
        if provider.should_trigger(&context) {
            let suggestions = provider.get_suggestions(&context);
            if !suggestions.is_empty() {
                self.overlays.push(Box::new(crate::overlays::autocomplete::AutocompleteOverlay::new(suggestions)));
                return;
            }
        }
    }
}
```

- [ ] **Step 3: Update submit_input to use Editor**

```rust
fn submit_input(&mut self) {
    let text = self.editor.take_text();
    if text.trim().is_empty() {
        return;
    }
    
    // Expand paste markers
    let text = self.paste_store.resolve_markers(&text);
    
    // Check for command
    if text.starts_with('/') {
        if let Some(cmd) = crate::command::Command::parse(&text) {
            self.handle_command(cmd);
            return;
        }
    }
    
    // Add user message and send (existing logic)
    // ... rest of method unchanged ...
}
```

- [ ] **Step 4: Update render_ui**

Replace `self.input.render(...)` with `self.editor.render(...)`.

- [ ] **Step 5: Verify and commit**

```bash
git add crates/tui/src/app.rs
git commit -m "feat(tui): integrate Editor, KeybindingsManager, and autocomplete into App"
```

---

## Phase 6: Config Update (P0)

### Task 6.1: Update KeysConfig

**Files:**
- Modify: `crates/tui/src/config.rs`

**Steps:**

- [ ] **Step 1: Update KeysConfig struct**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeysConfig {
    #[serde(flatten)]
    pub bindings: HashMap<String, toml::Value>,
}
```

- [ ] **Step 2: Add config loading tests**

```rust
#[test]
fn test_keys_config_parsing() {
    let toml_str = r#"
[server]
url = "http://localhost:8080"

[keys]
"editor.submit" = "ctrl+enter"
"editor.new_line" = ["shift+enter", "alt+enter"]
"#;
    let config: Config = toml::from_str(toml_str).expect("parse config");
    let keys = config.keys.expect("keys config");
    assert!(keys.bindings.contains_key("editor.submit"));
    assert!(keys.bindings.contains_key("editor.new_line"));
}
```

- [ ] **Step 3: Verify and commit**

```bash
git add crates/tui/src/config.rs
git commit -m "feat(tui): add KeysConfig with TOML parsing"
```

---

## Phase 7: Help Overlay Update (P1)

### Task 7.1: Update HelpOverlay to show keybindings from KeybindingsManager

**Files:**
- Modify: `crates/tui/src/overlays/help.rs`

**Steps:**

- [ ] **Step 1: Update HelpOverlay to read dynamic keybindings**

Modify HelpOverlay::new() to accept a reference to KeybindingsManager and display actual bindings:

```rust
pub struct HelpOverlay {
    lines: Vec<Line<'static>>,
}

impl HelpOverlay {
    pub fn new(keybindings: &crate::keybindings::KeybindingsManager) -> Self {
        let mut lines = vec![
            Line::from(Span::styled("Keybindings", Style::default().add_modifier(Modifier::BOLD))),
            Line::from(""),
        ];
        
        // Build lines from keybindings manager
        let bindings = vec![
            (Keybinding::EditorSubmit, "Submit message"),
            (Keybinding::EditorNewLine, "Insert newline"),
            (Keybinding::AppInterrupt, "Cancel / interrupt"),
            (Keybinding::AppQuit, "Quit"),
            (Keybinding::AppToggleToolCalls, "Toggle tool calls"),
            (Keybinding::AppToggleThinking, "Toggle thinking blocks"),
            (Keybinding::AppSelectModel, "Select model"),
            (Keybinding::AppListSessions, "List sessions"),
            (Keybinding::EditorCursorWordLeft, "Previous word"),
            (Keybinding::EditorCursorWordRight, "Next word"),
            (Keybinding::EditorDeleteWordBackward, "Delete word backward"),
            (Keybinding::EditorDeleteToLineEnd, "Delete to line end"),
            (Keybinding::EditorYank, "Yank from kill ring"),
            (Keybinding::EditorUndo, "Undo"),
        ];
        
        for (binding, desc) in bindings {
            let keys = keybindings.get_binding_keys(binding);
            let key_str = keys.join(", ");
            lines.push(Line::from(vec![
                Span::styled(format!("{:20}", key_str), Style::default().fg(theme.accent)),
                Span::styled(desc, Style::default().fg(theme.text)),
            ]));
        }
        
        Self { lines }
    }
}
```

- [ ] **Step 2: Verify and commit**

```bash
git add crates/tui/src/overlays/help.rs
git commit -m "feat(tui): update HelpOverlay with dynamic keybindings"
```

---

## Phase 8: Testing & Integration

### Task 8.1: Update integration tests

**Files:**
- Modify: `crates/tui/tests/integration.rs`

**Steps:**

- [ ] **Step 1: Add keybindings tests**

```rust
#[test]
fn test_keybindings_manager() {
    let kb = tui::keybindings::KeybindingsManager::new();
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    
    let enter_event = KeyEvent::from(KeyCode::Enter);
    assert!(kb.matches(&enter_event, tui::keybindings::Keybinding::EditorSubmit));
    
    let ctrl_c = KeyEvent {
        code: KeyCode::Char('c'),
        modifiers: KeyModifiers::CONTROL,
        kind: crossterm::event::KeyEventKind::Press,
        state: crossterm::event::KeyEventState::empty(),
    };
    assert!(kb.matches(&ctrl_c, tui::keybindings::Keybinding::AppQuit));
}
```

- [ ] **Step 2: Add editor tests**

```rust
#[test]
fn test_editor_multi_line() {
    let mut ed = tui::widgets::editor::Editor::new();
    ed.insert_text("line1");
    ed.insert_newline();
    ed.insert_text("line2");
    assert_eq!(ed.take_text(), "line1\nline2");
}
```

- [ ] **Step 3: Run all tests**

```bash
cargo test -p tui
```
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/tui/tests/integration.rs
git commit -m "test(tui): add integration tests for keybindings and editor"
```

### Task 8.2: Final verification

- [ ] **Step 1: Run clippy**

```bash
cargo clippy -p tui -- -D warnings
```
Expected: No warnings

- [ ] **Step 2: Run full test suite**

```bash
cargo test -p tui
```
Expected: All tests pass

- [ ] **Step 3: Build check**

```bash
cargo check -p tui
```
Expected: Compiles cleanly

- [ ] **Step 4: Final commit**

```bash
git commit -m "chore(tui): P0/P1 implementation complete - editor, keybindings, autocomplete"
```

---

## Summary of Changes

### New Files
- `src/keybindings.rs` — Global keybinding registry
- `src/autocomplete.rs` — Autocomplete provider trait and implementations
- `src/widgets/editor.rs` — Multi-line editor (replaces input_bar.rs)
- `src/overlays/autocomplete.rs` — Suggestion list overlay

### Modified Files
- `src/app.rs` — Integrated KeybindingsManager, Editor, autocomplete
- `src/main.rs` — Enabled BracketedPaste
- `src/config.rs` — Added KeysConfig TOML parsing
- `src/paste.rs` — Added resolve_markers()
- `src/overlays/help.rs` — Dynamic keybindings display
- `src/overlays/mod.rs` — Registered autocomplete module
- `src/widgets/mod.rs` — Replaced input_bar with editor
- `src/widgets/chat_view.rs` — Minor updates for Editor
- `tests/integration.rs` — Added keybindings and editor tests

### Deleted Files
- `src/widgets/input_bar.rs` — Replaced by Editor
