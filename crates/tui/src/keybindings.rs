use crate::config::KeysConfig;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

pub type KeyId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keybinding {
    AppQuit,
    AppInterrupt,
    AppToggleToolCalls,
    AppToggleThinking,
    AppSelectModel,
    AppListSessions,
    AppNewSession,
    AppOpenCommandPalette,
    AppExternalEditor,
    AppRestoreQueue,
    AppCycleModelForward,
    AppCycleModelBackward,
    EditorCursorUp,
    EditorCursorDown,
    EditorCursorLeft,
    EditorCursorRight,
    EditorCursorWordLeft,
    EditorCursorWordRight,
    EditorCursorLineStart,
    EditorCursorLineEnd,
    EditorPageUp,
    EditorPageDown,
    EditorDeleteCharBackward,
    EditorDeleteCharForward,
    EditorDeleteWordBackward,
    EditorDeleteWordForward,
    EditorDeleteToLineStart,
    EditorDeleteToLineEnd,
    EditorNewLine,
    EditorSubmit,
    EditorUndo,
    EditorRedo,
    EditorYank,
    EditorYankPop,
    EditorCharJump,
    AutocompleteTrigger,
}

pub fn key_event_to_id(event: &KeyEvent) -> KeyId {
    let key_str = match event.code {
        KeyCode::Backspace => "backspace",
        KeyCode::Enter => "enter",
        KeyCode::Left => "left",
        KeyCode::Right => "right",
        KeyCode::Up => "up",
        KeyCode::Down => "down",
        KeyCode::Home => "home",
        KeyCode::End => "end",
        KeyCode::PageUp => "pageup",
        KeyCode::PageDown => "pagedown",
        KeyCode::Tab => "tab",
        KeyCode::BackTab => "backtab",
        KeyCode::Delete => "delete",
        KeyCode::Insert => "insert",
        KeyCode::F(n) => return format!("f{n}"),
        KeyCode::Esc => "esc",
        KeyCode::Null => return String::new(),
        KeyCode::Char(c) => return format_modifier_key(c.to_lowercase().to_string(), event.modifiers),
        _ => return String::new(),
    };

    format_modifier_key(key_str.to_string(), event.modifiers)
}

fn format_modifier_key(key: String, modifiers: KeyModifiers) -> KeyId {
    let mut parts: Vec<&str> = Vec::new();

    if modifiers.contains(KeyModifiers::ALT) {
        parts.push("alt");
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl");
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("shift");
    }

    if parts.is_empty() {
        key
    } else {
        parts.push(&key);
        parts.join("+")
    }
}

pub fn default_keybindings() -> HashMap<Keybinding, Vec<KeyId>> {
    let mut m = HashMap::new();

    m.insert(Keybinding::AppQuit, vec!["ctrl+c".into(), "ctrl+d".into()]);
    m.insert(Keybinding::AppInterrupt, vec!["esc".into()]);
    m.insert(Keybinding::AppToggleToolCalls, vec!["ctrl+o".into()]);
    m.insert(Keybinding::AppToggleThinking, vec!["ctrl+t".into()]);
    m.insert(Keybinding::AppSelectModel, vec!["ctrl+l".into()]);
    m.insert(Keybinding::AppListSessions, vec!["ctrl+s".into()]);
    m.insert(Keybinding::AppNewSession, vec!["ctrl+n".into()]);
    m.insert(
        Keybinding::EditorSubmit,
        vec!["enter".into(), "alt+enter".into()],
    );
    m.insert(
        Keybinding::EditorNewLine,
        vec!["shift+enter".into()],
    );
    m.insert(Keybinding::EditorCursorUp, vec!["up".into()]);
    m.insert(Keybinding::EditorCursorDown, vec!["down".into()]);
    m.insert(
        Keybinding::EditorCursorLeft,
        vec!["left".into(), "ctrl+b".into()],
    );
    m.insert(
        Keybinding::EditorCursorRight,
        vec!["right".into(), "ctrl+f".into()],
    );
    m.insert(
        Keybinding::EditorCursorWordLeft,
        vec!["alt+left".into(), "ctrl+left".into(), "alt+b".into()],
    );
    m.insert(
        Keybinding::EditorCursorWordRight,
        vec!["alt+right".into(), "ctrl+right".into(), "alt+f".into()],
    );
    m.insert(
        Keybinding::EditorCursorLineStart,
        vec!["home".into(), "ctrl+a".into()],
    );
    m.insert(
        Keybinding::EditorCursorLineEnd,
        vec!["end".into(), "ctrl+e".into()],
    );
    m.insert(Keybinding::EditorPageUp, vec!["pageup".into()]);
    m.insert(Keybinding::EditorPageDown, vec!["pagedown".into()]);
    m.insert(
        Keybinding::EditorDeleteCharBackward,
        vec!["backspace".into()],
    );
    m.insert(
        Keybinding::EditorDeleteCharForward,
        vec!["delete".into(), "ctrl+d".into()],
    );
    m.insert(
        Keybinding::EditorDeleteWordBackward,
        vec!["ctrl+w".into(), "alt+backspace".into()],
    );
    m.insert(
        Keybinding::EditorDeleteWordForward,
        vec!["alt+d".into(), "alt+delete".into()],
    );
    m.insert(
        Keybinding::EditorDeleteToLineStart,
        vec!["ctrl+u".into()],
    );
    m.insert(
        Keybinding::EditorDeleteToLineEnd,
        vec!["ctrl+k".into()],
    );
    m.insert(Keybinding::EditorYank, vec!["ctrl+y".into()]);
    m.insert(Keybinding::EditorYankPop, vec!["alt+y".into()]);
    m.insert(Keybinding::EditorCharJump, vec!["ctrl+]".into()]);
    m.insert(Keybinding::EditorUndo, vec!["ctrl+-".into()]);
    m.insert(Keybinding::EditorRedo, vec!["ctrl+shift+-".into()]);
    m.insert(Keybinding::AutocompleteTrigger, vec!["tab".into()]);

    // AppOpenCommandPalette has no default binding
    m.insert(Keybinding::AppOpenCommandPalette, vec!["ctrl+shift+p".into()]);
    m.insert(Keybinding::AppExternalEditor, vec!["ctrl+x".into()]);
    m.insert(Keybinding::AppRestoreQueue, vec!["ctrl+u".into()]);
    m.insert(Keybinding::AppCycleModelForward, vec!["ctrl+shift+n".into()]);
    m.insert(Keybinding::AppCycleModelBackward, vec!["ctrl+p".into()]);

    m
}

#[derive(Debug)]
pub struct KeybindingsManager {
    defaults: HashMap<Keybinding, Vec<KeyId>>,
    user: HashMap<Keybinding, Vec<KeyId>>,
}

impl KeybindingsManager {
    pub fn new() -> Self {
        Self {
            defaults: default_keybindings(),
            user: HashMap::new(),
        }
    }

    pub fn load_user_config(&mut self, config: &KeysConfig) {
        self.user.clear();

        for (key, value) in &config.bindings {
            let binding = match Self::parse_keybinding_key(key) {
                Some(b) => b,
                None => continue,
            };

            let key_ids = match value {
                toml::Value::String(s) => vec![s.clone()],
                toml::Value::Array(arr) => arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
                _ => continue,
            };

            self.user.insert(binding, key_ids);
        }
    }

    pub fn matches(&self, event: &KeyEvent, binding: Keybinding) -> bool {
        let key_id = key_event_to_id(event);
        if key_id.is_empty() {
            return false;
        }

        if let Some(user_keys) = self.user.get(&binding) {
            user_keys.contains(&key_id)
        } else {
            self.defaults.get(&binding).map_or(false, |keys| keys.contains(&key_id))
        }
    }

    pub fn get_binding_keys(&self, binding: Keybinding) -> Vec<KeyId> {
        if let Some(user_keys) = self.user.get(&binding) {
            user_keys.clone()
        } else {
            self.defaults.get(&binding).cloned().unwrap_or_default()
        }
    }

    pub fn parse_keybinding_key(key: &str) -> Option<Keybinding> {
        match key {
            "app.quit" => Some(Keybinding::AppQuit),
            "app.interrupt" => Some(Keybinding::AppInterrupt),
            "app.toggle_tool_calls" => Some(Keybinding::AppToggleToolCalls),
            "app.toggle_thinking" => Some(Keybinding::AppToggleThinking),
            "app.select_model" => Some(Keybinding::AppSelectModel),
            "app.list_sessions" => Some(Keybinding::AppListSessions),
            "app.new_session" => Some(Keybinding::AppNewSession),
            "app.open_command_palette" => Some(Keybinding::AppOpenCommandPalette),
            "app.external_editor" => Some(Keybinding::AppExternalEditor),
            "app.restore_queue" => Some(Keybinding::AppRestoreQueue),
            "app.cycle_model_forward" => Some(Keybinding::AppCycleModelForward),
            "app.cycle_model_backward" => Some(Keybinding::AppCycleModelBackward),
            "editor.cursor_up" => Some(Keybinding::EditorCursorUp),
            "editor.cursor_down" => Some(Keybinding::EditorCursorDown),
            "editor.cursor_left" => Some(Keybinding::EditorCursorLeft),
            "editor.cursor_right" => Some(Keybinding::EditorCursorRight),
            "editor.cursor_word_left" => Some(Keybinding::EditorCursorWordLeft),
            "editor.cursor_word_right" => Some(Keybinding::EditorCursorWordRight),
            "editor.cursor_line_start" => Some(Keybinding::EditorCursorLineStart),
            "editor.cursor_line_end" => Some(Keybinding::EditorCursorLineEnd),
            "editor.page_up" => Some(Keybinding::EditorPageUp),
            "editor.page_down" => Some(Keybinding::EditorPageDown),
            "editor.delete_char_backward" => Some(Keybinding::EditorDeleteCharBackward),
            "editor.delete_char_forward" => Some(Keybinding::EditorDeleteCharForward),
            "editor.delete_word_backward" => Some(Keybinding::EditorDeleteWordBackward),
            "editor.delete_word_forward" => Some(Keybinding::EditorDeleteWordForward),
            "editor.delete_to_line_start" => Some(Keybinding::EditorDeleteToLineStart),
            "editor.delete_to_line_end" => Some(Keybinding::EditorDeleteToLineEnd),
            "editor.new_line" => Some(Keybinding::EditorNewLine),
            "editor.submit" => Some(Keybinding::EditorSubmit),
            "editor.undo" => Some(Keybinding::EditorUndo),
            "editor.redo" => Some(Keybinding::EditorRedo),
            "editor.yank" => Some(Keybinding::EditorYank),
            "editor.yank_pop" => Some(Keybinding::EditorYankPop),
            "editor.char_jump" => Some(Keybinding::EditorCharJump),
            "autocomplete.trigger" => Some(Keybinding::AutocompleteTrigger),
            _ => None,
        }
    }
}

impl Default for KeybindingsManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_event_to_id_simple() {
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(key_event_to_id(&event), "c");
    }

    #[test]
    fn test_key_event_to_id_ctrl() {
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_event_to_id(&event), "ctrl+c");
    }

    #[test]
    fn test_key_event_to_id_shift_enter() {
        let event = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(key_event_to_id(&event), "shift+enter");
    }

    #[test]
    fn test_matches_default_binding() {
        let manager = KeybindingsManager::new();
        let enter_key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(manager.matches(&enter_key, Keybinding::EditorSubmit));
    }

    #[test]
    fn test_user_override() {
        let mut manager = KeybindingsManager::new();

        let mut bindings = HashMap::new();
        bindings.insert("editor.submit".to_string(), toml::Value::String("ctrl+enter".to_string()));
        let config = KeysConfig { bindings };

        manager.load_user_config(&config);

        let enter_key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let ctrl_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL);

        assert!(!manager.matches(&enter_key, Keybinding::EditorSubmit));
        assert!(manager.matches(&ctrl_enter, Keybinding::EditorSubmit));
    }
}
