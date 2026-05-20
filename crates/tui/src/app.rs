use crate::autocomplete::{
    AutocompleteContext, AutocompleteProvider, FilePathProvider, SlashCommandProvider,
};
use crate::bash_mode;
use crate::client::model::{HistoricalMessage, ServerEvent, SessionInfo};
use crate::client::rest::RestClient;
use crate::client::sse;
use crate::command::Command;
use crate::component::{Component, OverlayResult};
use crate::config::Config;
use crate::input_queue::{InputQueue, QueueStrategy};
use crate::keybindings::{Keybinding, KeybindingsManager};
use crate::paste::PasteStore;
use crate::state::*;
use crate::ui::theme::Theme;
use crate::widgets::chat_view::render_chat;
use crate::widgets::editor::Editor;
use crate::widgets::header::HeaderBar;
use crate::widgets::pending_messages::PendingMessagesWidget;
use crate::widgets::session_tabs::SessionTabsWidget;
use crate::widgets::spinner::SpinnerWidget;
use crate::widgets::status_bar::render_status_bar;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::Widget;
use ratatui::Frame;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Return the pandaria temp directory, falling back to system temp_dir.
fn temp_dir_for_pandaria() -> std::path::PathBuf {
    if let Ok(root) = std::env::var("PANDARIA_SPACE_ROOT") {
        let dir = std::path::PathBuf::from(root).join("temp");
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }
    if let Ok(home) = std::env::var("HOME") {
        let dir = std::path::PathBuf::from(home).join(".pandaria").join("temp");
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }
    std::env::temp_dir()
}

/// Action sent from background tasks back to the main event loop.
pub enum TaskAction {
    SessionCreated(SessionInfo),
    ConnectionTested { url: String, ok: bool },
    SessionFetched(SessionInfo),
    SessionDeleted { id: String },
    HistoryLoaded { id: String, messages: Vec<HistoricalMessage> },
    BashCompleted {
        command: String,
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
        show_command: bool,
        text_to_send: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    Disconnected,
    Connected,
    Busy,
}

pub struct App {
    pub state: AppState,
    pub data: crate::state::State,
    pub config: Config,
    pub theme: Theme,
    pub rest: RestClient,
    pub editor: Editor,
    pub keybindings: KeybindingsManager,
    pub autocomplete_providers: Vec<Box<dyn AutocompleteProvider>>,
    pub spinner: SpinnerWidget,
    pub overlays: Vec<Box<dyn Component>>,
    pub header: HeaderBar,
    pub session_tabs: SessionTabsWidget,
    pub reqwest_client: reqwest::Client,
    pub paste_store: PasteStore,
    pub context_window: Option<u64>,
    pub input_tokens: u64,
    pub server_rx: Option<mpsc::Receiver<ServerEvent>>,
    pub sse_task: Option<tokio::task::JoinHandle<()>>,
    pub task_tx: mpsc::Sender<TaskAction>,
    pub task_rx: Option<mpsc::Receiver<TaskAction>>,
    pub scroll_offset: usize,
    pub user_scrolled_up: bool,
    pub running: bool,
    pub input_queue: InputQueue,
}

impl App {
    pub fn new(config: Config, session_id: String, session_info: SessionInfo) -> Self {
        let rest = RestClient::new(&config.server);
        let data = crate::state::State::new(session_id, session_info);
        let context_window = data.active_session().info.context_window;

        let mut keybindings = KeybindingsManager::new();
        if let Some(ref keys_config) = config.keys {
            keybindings.load_user_config(keys_config);
        }
        let (task_tx, task_rx) = mpsc::channel::<TaskAction>(32);
        let theme = Theme::default();
        Self {
            state: AppState::Connected,
            data,
            config,
            theme: theme.clone(),
            rest,
            editor: Editor::new(theme.clone()),
            keybindings,
            autocomplete_providers: vec![
                Box::new(SlashCommandProvider::new()),
                Box::new(FilePathProvider::new()),
            ],
            spinner: SpinnerWidget::new(),
            overlays: Vec::new(),
            header: HeaderBar::new(theme.clone()),
            session_tabs: SessionTabsWidget::new(theme),
            reqwest_client: reqwest::Client::new(),
            paste_store: PasteStore::new(),
            context_window,
            input_tokens: 0,
            server_rx: None,
            sse_task: None,
            task_tx,
            task_rx: Some(task_rx),
            scroll_offset: 0,
            user_scrolled_up: false,
            running: true,
            input_queue: InputQueue::new(),
        }
    }

    fn build_autocomplete_context(&self) -> AutocompleteContext {
        let lines = self.editor.lines.join("\n");
        AutocompleteContext {
            full_text: lines.clone(),
            cursor_line: self.editor.cursor_line,
            cursor_col: self.editor.cursor_col,
            current_line: self.editor.current_line_text().to_string(),
            text_before_cursor: self.editor.text_before_cursor(),
        }
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        // Overlays take priority
        if let Some(overlay) = self.overlays.last_mut() {
            if !overlay.is_capturing() {
                match key.code {
                    KeyCode::Char(_) => {
                        overlay.handle_input(key);
                        if matches!(overlay.take_result(), OverlayResult::Dismissed) {
                            self.overlays.pop();
                        }
                        // Continue to editor char input below
                    }
                    _ => {
                        overlay.handle_input(key);
                        match overlay.take_result() {
                            OverlayResult::Confirmed(value) => {
                                self.overlays.pop();
                                self.handle_overlay_confirm(value);
                            }
                            OverlayResult::Dismissed => {
                                self.overlays.pop();
                            }
                            OverlayResult::Pending => {}
                        }
                        return;
                    }
                }
            } else {
                overlay.handle_input(key);
                match overlay.take_result() {
                    OverlayResult::Confirmed(value) => {
                        self.overlays.pop();
                        self.handle_overlay_confirm(value);
                    }
                    OverlayResult::Dismissed => {
                        self.overlays.pop();
                    }
                    OverlayResult::Pending => {}
                }
                return;
            }
        }

        let kb = &self.keybindings;

        // --- App-level keybindings ---
        if kb.matches(&key, Keybinding::AppQuit) && self.state != AppState::Busy {
            self.running = false;
            return;
        }
        if kb.matches(&key, Keybinding::AppInterrupt) {
            if self.state == AppState::Busy {
                let rest = self.rest.clone();
                let token = self.config.auth.token.clone().unwrap_or_default();
                let sid = self.data.active_session.clone();
                tokio::spawn(async move {
                    if let Err(e) = rest.interrupt(&sid, &token).await {
                        tracing::error!("interrupt failed: {e}");
                    }
                });
                if let Some(last) = self.data.active_session_mut().messages.last_mut() {
                    last.status = MessageStatus::Aborted;
                }
                self.state = AppState::Connected;
            } else {
                self.editor.clear();
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
            self.open_model_selector();
            return;
        }
        if kb.matches(&key, Keybinding::AppListSessions) {
            let sessions: Vec<_> = self.data.sessions.iter()
                .map(|(id, s)| (id.clone(), s.info.title.clone().unwrap_or_else(|| id.chars().take(8).collect())))
                .collect();
            self.overlays.push(Box::new(
                crate::overlays::session_list::SessionListOverlay::new(sessions),
            ));
            return;
        }
        if kb.matches(&key, Keybinding::AppNewSession) && self.state != AppState::Busy {
            let rest = self.rest.clone();
            let token = self.config.auth.token.clone().unwrap_or_default();
            let task_tx = self.task_tx.clone();
            tokio::spawn(async move {
                match rest.create_session(None, &token).await {
                    Ok(info) => {
                        tracing::info!(session_id = %info.id, "created new session");
                        let _ = task_tx.send(TaskAction::SessionCreated(info)).await;
                    }
                    Err(e) => tracing::error!("create session failed: {e}"),
                }
            });
            return;
        }

        // --- Editor keybindings ---
        if kb.matches(&key, Keybinding::EditorSubmit) {
            let force_steer = key.modifiers.contains(KeyModifiers::ALT);
            self.submit_input(force_steer);
            return;
        }
        if kb.matches(&key, Keybinding::EditorNewLine) {
            self.editor.insert_newline();
            return;
        }
        if kb.matches(&key, Keybinding::EditorCursorUp) {
            if self.editor.cursor_line == 0 {
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
        if kb.matches(&key, Keybinding::EditorCharJump) {
            // Enter char-jump mode: next character typed will be the jump target
            self.editor.set_char_jump_target('\0'); // placeholder, will be overwritten by next char
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
        if kb.matches(&key, Keybinding::EditorRedo) { self.editor.redo(); return; }
        if kb.matches(&key, Keybinding::AutocompleteTrigger) {
            let ctx = self.build_autocomplete_context();
            for provider in &self.autocomplete_providers {
                if provider.should_trigger(&ctx) {
                    let suggestions = provider.get_suggestions(&ctx);
                    if !suggestions.is_empty() {
                        self.overlays.push(Box::new(
                            crate::overlays::autocomplete::AutocompleteOverlay::new(suggestions),
                        ));
                    }
                    return;
                }
            }
            // No provider triggered — insert a literal tab (or spaces)
            self.editor.insert_text("    ");
            return;
        }
        if kb.matches(&key, Keybinding::AppOpenCommandPalette) {
            let snapshot = self.editor.lines.join("\n");
            self.input_queue.snapshot_editor(snapshot);
            self.overlays.push(Box::new(
                crate::overlays::command_palette::CommandPalette::new(),
            ));
            return;
        }
        if kb.matches(&key, Keybinding::AppExternalEditor) {
            self.open_external_editor();
            return;
        }
        if kb.matches(&key, Keybinding::AppRestoreQueue) {
            if let Some(text) = self.input_queue.restore_to_editor() {
                self.editor.clear();
                self.editor.insert_text(&text);
            }
            return;
        }
        if kb.matches(&key, Keybinding::AppCycleModelForward) {
            self.cycle_model(true);
            return;
        }
        if kb.matches(&key, Keybinding::AppCycleModelBackward) {
            self.cycle_model(false);
            return;
        }

        // Char input
        if let KeyCode::Char(ch) = key.code {
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                if self.editor.is_waiting_char_jump() {
                    self.editor.set_char_jump_target(ch);
                    self.editor.char_jump();
                } else {
                    self.editor.insert_char(ch);
                }
            }
        }
    }

    /// Check top overlay for confirmed/dismissed flags and process accordingly.
    fn handle_overlay_confirm(&mut self, value: String) {
        if value.starts_with("delete:") {
            let id = value.trim_start_matches("delete:").to_string();
            let rest = self.rest.clone();
            let token = self.config.auth.token.clone().unwrap_or_default();
            let task_tx = self.task_tx.clone();
            tokio::spawn(async move {
                match rest.delete_session(&id, &token).await {
                    Ok(()) => {
                        let _ = task_tx.send(TaskAction::SessionDeleted { id }).await;
                    }
                    Err(e) => tracing::warn!("delete session failed: {e}"),
                }
            });
            return;
        }
        if let Some(cmd) = Command::parse(&value) {
            match cmd {
                Command::Quit => self.running = false,
                Command::Help => {
                    self.overlays
                        .push(Box::new(crate::overlays::help::HelpOverlay::new()));
                }
                Command::Clear => {
                    self.data.active_session_mut().messages.clear();
                }
                Command::NewSession { title } => {
                    let rest = self.rest.clone();
                    let token = self.config.auth.token.clone().unwrap_or_default();
                    let task_tx = self.task_tx.clone();
                    tokio::spawn(async move {
                        match rest.create_session(title.as_deref(), &token).await {
                            Ok(info) => {
                                tracing::info!(session_id = %info.id, "created new session");
                                let _ = task_tx.send(TaskAction::SessionCreated(info)).await;
                            }
                            Err(e) => tracing::error!("create session failed: {e}"),
                        }
                    });
                }
                Command::SwitchSession { id } => {
                    if self.data.sessions.contains_key(&id) {
                        self.data.active_session = id.clone();
                        if let Some(s) = self.data.sessions.get(&id) {
                            self.context_window = s.info.context_window;
                        }
                        let rest = self.rest.clone();
                        let token = self.config.auth.token.clone().unwrap_or_default();
                        let task_tx = self.task_tx.clone();
                        let sid = id.clone();
                        tokio::spawn(async move {
                            match rest.get_session(&sid, &token).await {
                                Ok(info) => {
                                    let _ = task_tx.send(TaskAction::SessionFetched(info)).await;
                                }
                                Err(e) => tracing::warn!("switch session fetch failed: {e}"),
                            }
                            match rest.get_session_messages(&sid, &token).await {
                                Ok(messages) => {
                                    let _ = task_tx.send(TaskAction::HistoryLoaded { id: sid, messages }).await;
                                }
                                Err(e) => tracing::warn!("fetch history failed: {e}"),
                            }
                        });
                    }
                }
                Command::ListSessions => {
                    let sessions: Vec<_> = self
                        .data
                        .sessions
                        .iter()
                        .map(|(id, s)| {
                            (
                                id.clone(),
                                s.info
                                    .title
                                    .clone()
                                    .unwrap_or_else(|| id.chars().take(8).collect()),
                            )
                        })
                        .collect();
                    self.overlays.push(Box::new(
                        crate::overlays::session_list::SessionListOverlay::new(sessions),
                    ));
                }
                Command::SelectModel { id } => {
                    if let Some(model_id) = id {
                        let rest = self.rest.clone();
                        let token = self.config.auth.token.clone().unwrap_or_default();
                        let sid = self.data.active_session().info.id.clone();
                        let task_tx = self.task_tx.clone();
                        let model_id_clone = model_id.clone();
                        tokio::spawn(async move {
                            match rest.update_model(&sid, &model_id_clone, &token).await {
                                Ok(info) => {
                                    let _ = task_tx.send(TaskAction::SessionFetched(info)).await;
                                }
                                Err(e) => tracing::warn!("update model failed: {e}"),
                            }
                        });
                        let session = self.data.active_session_mut();
                        session.info.model = model_id;
                    } else {
                        self.open_model_selector();
                    }
                }
                Command::Connect { url } => {
                    self.config.server.url = url.clone();
                    let rest = RestClient::new(&self.config.server);
                    let token = self.config.auth.token.clone().unwrap_or_default();
                    let task_tx = self.task_tx.clone();
                    tokio::spawn(async move {
                        let ok = rest.list_sessions(&token).await.is_ok();
                        let _ = task_tx.send(TaskAction::ConnectionTested { url, ok }).await;
                    });
                }
                Command::Auth { token } => {
                    self.config.auth.token = Some(token.clone());
                    let rest = RestClient::new(&self.config.server);
                    let task_tx = self.task_tx.clone();
                    tokio::spawn(async move {
                        let ok = rest.list_sessions(&token).await.is_ok();
                        let _ = task_tx.send(TaskAction::ConnectionTested { url: "(auth)".to_string(), ok }).await;
                    });
                }
                Command::Tokens => {
                    let input = self.input_tokens;
                    let window = self.context_window;
                    let pct = window.map(|w| if w > 0 { (input * 100 / w).min(100) } else { 0 }).unwrap_or(0);
                    let msg = RenderedMessage {
                        role: MessageRole::Assistant,
                        blocks: vec![MessageBlock::Text(vec![ratatui::text::Line::from(
                            format!("Tokens: {input} / {} ({}%)", window.map(|w| w.to_string()).unwrap_or_else(|| "?".to_string()), pct)
                        )])],
                        timestamp: std::time::SystemTime::now(),
                        status: MessageStatus::Complete,
                    };
                    self.data.active_session_mut().messages.push(msg);
                }
                Command::Retry => {
                    let session = self.data.active_session();
                    let last_user_text = session.messages.iter().rev().find_map(|m| {
                        if m.role == MessageRole::User {
                            m.blocks.iter().find_map(|b| match b {
                                MessageBlock::Text(lines) => Some(lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n")),
                                _ => None,
                            })
                        } else {
                            None
                        }
                    });
                    if let Some(text) = last_user_text {
                        self.start_streaming_turn(text);
                    }
                }
                Command::Copy => {
                    let session = self.data.active_session();
                    let last_assistant_text: Option<String> = session.messages.iter().rev().find_map(|m| {
                        if m.role == MessageRole::Assistant && m.status == MessageStatus::Complete {
                            let text: String = m.blocks.iter().filter_map(|b| match b {
                                MessageBlock::Text(lines) => Some(lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n")),
                                _ => None,
                            }).collect::<Vec<_>>().join("\n");
                            Some(text)
                        } else {
                            None
                        }
                    });
                    if let Some(text) = last_assistant_text {
                        if let Err(e) = crate::clipboard::copy_text(&text) {
                            self.data.last_error = Some(e);
                        }
                    }
                }
                Command::Dump { filename } => {
                    let session = self.data.active_session();
                    let mut md = String::new();
                    md.push_str(&format!("# Session: {}\n\n", session.info.id));
                    for msg in &session.messages {
                        match msg.role {
                            MessageRole::User => {
                                md.push_str("## User\n\n");
                                for block in &msg.blocks {
                                    match block {
                                        MessageBlock::Text(lines) => {
                                            let text = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n");
                                            md.push_str(&text);
                                            md.push('\n');
                                        }
                                        MessageBlock::BashExecution(be) => {
                                            md.push_str(&format!("```bash\n$ {}\n{}\n```\n", be.command, be.stdout));
                                        }
                                        _ => {}
                                    }
                                }
                                md.push('\n');
                            }
                            MessageRole::Assistant => {
                                md.push_str("## Assistant\n\n");
                                for block in &msg.blocks {
                                    match block {
                                        MessageBlock::Text(lines) => {
                                            let text = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n");
                                            md.push_str(&text);
                                            md.push('\n');
                                        }
                                        MessageBlock::ToolCall(tc) => {
                                            md.push_str(&format!("```tool\n{}: {}\n```\n", tc.name, tc.call_id));
                                        }
                                        MessageBlock::Thinking(tb) => {
                                            md.push_str(&format!("```thinking\n{}\n```\n", tb.thinking_text));
                                        }
                                        MessageBlock::BashExecution(be) => {
                                            md.push_str(&format!("```bash\n$ {}\n{}\n```\n", be.command, be.stdout));
                                        }
                                        MessageBlock::CompactionSummary(cs) => {
                                            md.push_str(&format!("```compaction\n{}\n```\n", cs.summary));
                                        }
                                    }
                                }
                                md.push('\n');
                            }
                        }
                    }
                    let filename = filename.unwrap_or_else(|| format!("session_{}.md", session.info.id));
                    if let Err(e) = std::fs::write(&filename, md) {
                        self.data.last_error = Some(format!("dump failed: {e}"));
                    }
                }
                Command::Compact => {
                    let rest = self.rest.clone();
                    let token = self.config.auth.token.clone().unwrap_or_default();
                    let sid = self.data.active_session.clone();
                    tokio::spawn(async move {
                        if let Err(e) = rest.compact_session(&sid, &token).await {
                            tracing::warn!("compact failed: {e}");
                        }
                    });
                }
                Command::Rename { title } => {
                    let rest = self.rest.clone();
                    let token = self.config.auth.token.clone().unwrap_or_default();
                    let sid = self.data.active_session.clone();
                    let task_tx = self.task_tx.clone();
                    tokio::spawn(async move {
                        match rest.rename_session(&sid, &title, &token).await {
                            Ok(info) => {
                                let _ = task_tx.send(TaskAction::SessionFetched(info)).await;
                            }
                            Err(e) => tracing::warn!("rename failed: {e}"),
                        }
                    });
                }
                Command::Tree => {
                    // Placeholder: display linear message list as tree preview
                    let session = self.data.active_session();
                    let tree_text = session
                        .messages
                        .iter()
                        .enumerate()
                        .map(|(i, m)| {
                            let role = match m.role {
                                MessageRole::User => "User",
                                MessageRole::Assistant => "Assistant",
                            };
                            format!("{}: {}", i, role)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let msg = RenderedMessage {
                        role: MessageRole::Assistant,
                        blocks: vec![MessageBlock::Text(vec![ratatui::text::Line::from(
                            format!("Session tree:\n{}", tree_text),
                        )])],
                        timestamp: std::time::SystemTime::now(),
                        status: MessageStatus::Complete,
                    };
                    self.data.active_session_mut().messages.push(msg);
                }
                Command::Fork { message_id: _ } => {
                    // Placeholder: fork is not yet implemented in backend
                    self.data.last_error = Some("Fork is not yet implemented".to_string());
                }
                Command::Settings => {
                    // Placeholder: open settings overlay
                    self.data.last_error = Some("Settings overlay not yet implemented".to_string());
                }
                Command::Export { filename } => {
                    let session = self.data.active_session();
                    let mut md = String::new();
                    md.push_str(&format!("# Session: {}\n\n", session.info.id));
                    for msg in &session.messages {
                        match msg.role {
                            MessageRole::User => {
                                md.push_str("## User\n\n");
                                for block in &msg.blocks {
                                    match block {
                                        MessageBlock::Text(lines) => {
                                            let text = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n");
                                            md.push_str(&text);
                                            md.push('\n');
                                        }
                                        MessageBlock::ToolCall(tc) => {
                                            md.push_str(&format!("```tool\n{}: {}\n```\n", tc.name, tc.call_id));
                                        }
                                        MessageBlock::Thinking(tb) => {
                                            md.push_str(&format!("```thinking\n{}\n```\n", tb.thinking_text));
                                        }
                                        MessageBlock::BashExecution(be) => {
                                            md.push_str(&format!("```bash\n$ {}\n{}\n```\n", be.command, be.stdout));
                                        }
                                        MessageBlock::CompactionSummary(cs) => {
                                            md.push_str(&format!("```compaction\n{}\n```\n", cs.summary));
                                        }
                                    }
                                }
                                md.push('\n');
                            }
                            MessageRole::Assistant => {
                                md.push_str("## Assistant\n\n");
                                for block in &msg.blocks {
                                    match block {
                                        MessageBlock::Text(lines) => {
                                            let text = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n");
                                            md.push_str(&text);
                                            md.push('\n');
                                        }
                                        MessageBlock::ToolCall(tc) => {
                                            md.push_str(&format!("```tool\n{}: {}\n```\n", tc.name, tc.call_id));
                                        }
                                        MessageBlock::Thinking(tb) => {
                                            md.push_str(&format!("```thinking\n{}\n```\n", tb.thinking_text));
                                        }
                                        MessageBlock::BashExecution(be) => {
                                            md.push_str(&format!("```bash\n$ {}\n{}\n```\n", be.command, be.stdout));
                                        }
                                        MessageBlock::CompactionSummary(cs) => {
                                            md.push_str(&format!("```compaction\n{}\n```\n", cs.summary));
                                        }
                                    }
                                }
                                md.push('\n');
                            }
                        }
                    }
                    let filename = filename.unwrap_or_else(|| format!("session_{}.md", session.info.id));
                    if let Err(e) = std::fs::write(&filename, md) {
                        self.data.last_error = Some(format!("export failed: {e}"));
                    }
                }
                Command::Import { filename } => {
                    match std::fs::read_to_string(&filename) {
                        Ok(content) => {
                            let msg = RenderedMessage {
                                role: MessageRole::Assistant,
                                blocks: vec![MessageBlock::Text(vec![ratatui::text::Line::from(
                                    format!("Imported {} ({} bytes)", filename, content.len()),
                                )])],
                                timestamp: std::time::SystemTime::now(),
                                status: MessageStatus::Complete,
                            };
                            self.data.active_session_mut().messages.push(msg);
                        }
                        Err(e) => {
                            self.data.last_error = Some(format!("import failed: {e}"));
                        }
                    }
                }
                Command::DeleteSession => {
                    let id = self.data.active_session.clone();
                    let rest = self.rest.clone();
                    let token = self.config.auth.token.clone().unwrap_or_default();
                    let task_tx = self.task_tx.clone();
                    tokio::spawn(async move {
                        match rest.delete_session(&id, &token).await {
                            Ok(()) => {
                                let _ = task_tx.send(TaskAction::SessionDeleted { id }).await;
                            }
                            Err(e) => tracing::warn!("delete session failed: {e}"),
                        }
                    });
                }
                Command::SystemPrompt { prompt } => {
                    let rest = self.rest.clone();
                    let token = self.config.auth.token.clone().unwrap_or_default();
                    let sid = self.data.active_session.clone();
                    let task_tx = self.task_tx.clone();
                    tokio::spawn(async move {
                        match rest.update_system_prompt(&sid, &prompt, &token).await {
                            Ok(info) => {
                                let _ = task_tx.send(TaskAction::SessionFetched(info)).await;
                            }
                            Err(e) => tracing::warn!("update system prompt failed: {e}"),
                        }
                    });
                }
                Command::Skill { name } => {
                    if !name.is_empty() {
                        self.send_user_message(format!("/skill:{}", name));
                    }
                }
            }
        } else if self.config.ui.models.contains(&value) {
            // ModelSelector overlay returns a raw model name (not a /command)
            let rest = self.rest.clone();
            let token = self.config.auth.token.clone().unwrap_or_default();
            let sid = self.data.active_session().info.id.clone();
            let task_tx = self.task_tx.clone();
            let model = value.clone();
            tokio::spawn(async move {
                match rest.update_model(&sid, &model, &token).await {
                    Ok(info) => {
                        let _ = task_tx.send(TaskAction::SessionFetched(info)).await;
                    }
                    Err(e) => tracing::warn!("update model failed: {e}"),
                }
            });
            self.data.active_session_mut().info.model = value;
        }
    }

    pub fn handle_paste(&mut self, data: String) {
        let result = self.paste_store.store(&data);
        self.editor.insert_text(&result);
    }

    fn start_streaming_turn(&mut self, content: String) {
        let rest = self.rest.clone();
        let token = self.config.auth.token.clone().unwrap_or_default();
        let sid = self.data.active_session.clone();

        let (tx, rx) = mpsc::channel::<ServerEvent>(32);
        let reqwest_client = self.reqwest_client.clone();
        let base_url = self.config.server.url.clone();

        if let Some(handle) = self.sse_task.take() {
            handle.abort();
        }

        let sid_clone = sid.clone();
        let token_clone = token.clone();
        let sse_handle = tokio::spawn(async move {
            sse::connect(&reqwest_client, &base_url, &sid_clone, &token_clone, tx).await;
        });
        self.sse_task = Some(sse_handle);

        tokio::spawn(async move {
            if let Err(e) = rest.send_message(&sid, &content, &token).await {
                tracing::error!("send message failed: {e}");
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

    fn submit_input(&mut self, force_steer: bool) {
        let text = self.editor.take_text();
        if text.trim().is_empty() {
            return;
        }

        if text.starts_with('/') {
            self.handle_overlay_confirm(text);
            return;
        }

        // Detect bash mode
        let is_bash = bash_mode::detect_bash_mode(&text).is_some();
        let show_command = is_bash && bash_mode::is_double_bang(&text);
        let bash_command = if is_bash {
            bash_mode::detect_bash_mode(&text).map(|s| s.to_string())
        } else {
            None
        };

        let text = self.paste_store.expand(&text);

        // If busy, handle via input queue
        if self.state == AppState::Busy {
            let use_steer = force_steer || self.input_queue.strategy() == QueueStrategy::Steer;
            if use_steer {
                // Steer: interrupt current turn and send immediately
                self.perform_steer(text, is_bash, bash_command, show_command);
            } else {
                // FollowUp: queue for later
                self.input_queue.enqueue(text, is_bash);
            }
            return;
        }

        // Normal send
        if is_bash {
            if let Some(cmd) = bash_command {
                self.spawn_bash_task(cmd, show_command);
                return;
            }
        }

        self.send_user_message(text);
    }

    fn send_user_message(&mut self, text: String) {
        let msg = RenderedMessage {
            role: MessageRole::User,
            blocks: vec![MessageBlock::Text(vec![ratatui::text::Line::from(
                text.clone(),
            )])],
            timestamp: std::time::SystemTime::now(),
            status: MessageStatus::Complete,
        };
        self.data.active_session_mut().messages.push(msg);

        let max_history = self.config.ui.max_history;
        let session = self.data.active_session_mut();
        while session.messages.len() > max_history {
            session.messages.remove(0);
        }

        self.start_streaming_turn(text);
    }

    fn spawn_bash_task(&self, command: String, show_command: bool) {
        let task_tx = self.task_tx.clone();
        tokio::spawn(async move {
            let result = bash_mode::execute_bash(&command).await;
            let text_to_send = bash_mode::format_for_send(&result, show_command);
            let _ = task_tx
                .send(TaskAction::BashCompleted {
                    command: result.command,
                    stdout: result.stdout,
                    stderr: result.stderr,
                    exit_code: result.exit_code,
                    show_command,
                    text_to_send,
                })
                .await;
        });
    }

    fn finalize_bash_message(
        &mut self,
        command: String,
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
        show_command: bool,
        text_to_send: String,
    ) {
        let msg = RenderedMessage {
            role: MessageRole::User,
            blocks: vec![MessageBlock::BashExecution(BashExecutionBlock {
                command,
                stdout,
                stderr,
                exit_code,
                expanded: show_command,
            })],
            timestamp: std::time::SystemTime::now(),
            status: MessageStatus::Complete,
        };
        self.data.active_session_mut().messages.push(msg);

        let max_history = self.config.ui.max_history;
        let session = self.data.active_session_mut();
        while session.messages.len() > max_history {
            session.messages.remove(0);
        }

        self.start_streaming_turn(text_to_send);
    }

    fn perform_steer(&mut self, text: String, is_bash: bool, bash_command: Option<String>, show_command: bool) {
        // Interrupt current turn
        let rest = self.rest.clone();
        let token = self.config.auth.token.clone().unwrap_or_default();
        let sid = self.data.active_session.clone();
        tokio::spawn(async move {
            if let Err(e) = rest.interrupt(&sid, &token).await {
                tracing::error!("steer interrupt failed: {e}");
            }
        });

        // Mark last assistant message as aborted
        if let Some(last) = self.data.active_session_mut().messages.last_mut() {
            if last.role == MessageRole::Assistant && last.status == MessageStatus::Streaming {
                last.status = MessageStatus::Aborted;
            }
        }
        self.data.active_session_mut().streaming = None;
        self.state = AppState::Connected;

        // Abort SSE task
        if let Some(handle) = self.sse_task.take() {
            handle.abort();
        }

        // Send the steer message
        if is_bash {
            if let Some(cmd) = bash_command {
                self.spawn_bash_task(cmd, show_command);
                return;
            }
        }
        self.send_user_message(text);
    }

    pub fn handle_server_event(&mut self, event: ServerEvent) {
        // Update connection status before borrowing session mutably
        match &event {
            ServerEvent::MessageStart { .. } => {
                self.data.connection_status = crate::state::ConnectionStatus::Connected;
            }
            ServerEvent::Error { code, .. }
                if code == "sse_reconnecting" || code == "sse_parse_error" =>
            {
                self.data.connection_status = crate::state::ConnectionStatus::Reconnecting;
            }
            _ => {}
        }

        let session = self.data.active_session_mut();
        match event {
            ServerEvent::MessageStart { .. } => {}
            ServerEvent::TextDelta { delta } => {
                if let Some(ref mut buf) = session.streaming {
                    buf.text_content.push_str(&delta);
                }
                if let Some(last) = session.messages.last_mut()
                    && let Some(ref buf) = session.streaming
                {
                        let line = ratatui::text::Line::from(buf.text_content.clone());
                        let mut found = false;
                        for block in &mut last.blocks {
                            if let MessageBlock::Text(lines) = block {
                                *lines = vec![line.clone()];
                                found = true;
                                break;
                            }
                        }
                    if !found {
                        last.blocks.push(MessageBlock::Text(vec![line]));
                    }
                }
                if !self.user_scrolled_up { self.scroll_offset = 0; }
            }
            ServerEvent::ThinkingDelta {
                content_index: _,
                delta,
            } => {
                if let Some(ref mut buf) = session.streaming {
                    buf.thinking_content.push_str(&delta);
                }
                if let Some(last) = session.messages.last_mut() {
                    let text = session
                        .streaming
                        .as_ref()
                        .map(|b| b.thinking_content.clone())
                        .unwrap_or_default();
                    let mut found = false;
                    for block in &mut last.blocks {
                        if let MessageBlock::Thinking(tb) = block {
                            tb.thinking_text = text.clone();
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        last.blocks.push(MessageBlock::Thinking(ThinkingBlock {
                            thinking_text: text,
                            is_expanded: false,
                            is_redacted: false,
                        }));
                    }
                }
                if !self.user_scrolled_up { self.scroll_offset = 0; }
            }
            ServerEvent::ToolCallStarted { call_id, name } => {
                let tc = ToolCallWidget {
                    call_id: call_id.clone(),
                    name,
                    state: ToolCallState::Pending,
                    content: vec![],
                    is_expanded: false,
                };
                if let Some(ref mut buf) = session.streaming {
                    buf.pending_tool_calls.push(tc.clone());
                }
                if let Some(last) = session.messages.last_mut() {
                    last.blocks.push(MessageBlock::ToolCall(tc));
                }
                if !self.user_scrolled_up { self.scroll_offset = 0; }
            }
            ServerEvent::ToolCallDelta { call_id, delta } => {
                if let Some(ref mut buf) = session.streaming {
                    buf.tool_arg_buffers
                        .entry(call_id.clone())
                        .or_default()
                        .push_str(&delta);
                }
            }
            ServerEvent::ToolCallDone {
                call_id,
                result,
                is_error,
            } => {
                if let Some(last) = session.messages.last_mut() {
                    for block in &mut last.blocks {
                        if let MessageBlock::ToolCall(tc) = block
                            && tc.call_id == call_id
                        {
                                tc.state = if is_error {
                                    ToolCallState::Error
                                } else {
                                    ToolCallState::Success
                                };
                                if let Some(ref r) = result {
                                    tc.content =
                                        vec![ratatui::text::Line::from(r.clone())];
                                }
                                break;
                        }
                    }
                }
            }
            ServerEvent::TurnEnd { usage, .. } => {
                if let Some(ref u) = usage {
                    self.input_tokens = u.input_tokens;
                }
                if let Some(last) = session.messages.last_mut() {
                    last.status = MessageStatus::Complete;
                }
                session.streaming = None;
                self.state = AppState::Connected;

                let max_history = self.config.ui.max_history;
                let s = self.data.active_session_mut();
                while s.messages.len() > max_history {
                    s.messages.remove(0);
                }

                // Auto-dequeue pending followUp messages
                if let Some(item) = self.input_queue.dequeue() {
                    if item.is_bash {
                        if let Some(cmd) = bash_mode::detect_bash_mode(&item.text) {
                            let show_command = bash_mode::is_double_bang(&item.text);
                            self.spawn_bash_task(cmd.to_string(), show_command);
                        }
                    } else {
                        self.send_user_message(item.text);
                    }
                }
            }
            ServerEvent::Error { code, message } => {
                if let Some(last) = session.messages.last_mut() {
                    last.status = MessageStatus::Error;
                }
                session.error = Some(crate::client::model::ApiError { code, message });
                session.streaming = None;
                self.state = AppState::Connected;
            }
        }
    }

    pub fn handle_task_action(&mut self, action: TaskAction) {
        match action {
            TaskAction::SessionCreated(info) => {
                let id = info.id.clone();
                self.data.sessions.insert(id.clone(), SessionState::new(info));
                self.data.active_session = id;
                if let Some(s) = self.data.sessions.get(&self.data.active_session) {
                    self.context_window = s.info.context_window;
                }
            }
            TaskAction::ConnectionTested { url, ok } => {
                if ok {
                    tracing::info!(%url, "connection validated");
                } else {
                    self.data.last_error = Some(format!("Could not connect to {}", url));
                }
            }
            TaskAction::SessionFetched(info) => {
                let id = info.id.clone();
                let cw = info.context_window;
                if let Some(existing) = self.data.sessions.get_mut(&id) {
                    existing.info = info;
                }
                if id == self.data.active_session {
                    self.context_window = cw;
                }
            }
            TaskAction::BashCompleted {
                command,
                stdout,
                stderr,
                exit_code,
                show_command,
                text_to_send,
            } => {
                self.finalize_bash_message(
                    command,
                    stdout,
                    stderr,
                    exit_code,
                    show_command,
                    text_to_send,
                );
            }
            TaskAction::SessionDeleted { id } => {
                self.data.sessions.remove(&id);
                if self.data.active_session == id {
                    self.data.active_session = self.data.sessions.keys().next().cloned().unwrap_or_default();
                    if let Some(s) = self.data.sessions.get(&self.data.active_session) {
                        self.context_window = s.info.context_window;
                    }
                }
            }
            TaskAction::HistoryLoaded { id, messages } => {
                if let Some(session) = self.data.sessions.get_mut(&id) {
                    session.messages = Self::convert_history(messages);
                }
            }
        }
    }

    fn convert_history(messages: Vec<HistoricalMessage>) -> Vec<RenderedMessage> {
        use crate::client::model::{HistoricalContent, HistoricalMessage};
        messages
            .into_iter()
            .map(|m| {
                let (role, blocks) = match m {
                    HistoricalMessage::User(u) => {
                        let lines: Vec<ratatui::text::Line> = u.content.into_iter().filter_map(|c| {
                            match c {
                                HistoricalContent::Text { text } => Some(ratatui::text::Line::from(text)),
                                _ => None,
                            }
                        }).collect();
                        (MessageRole::User, vec![MessageBlock::Text(lines)])
                    }
                    HistoricalMessage::Assistant(a) => {
                        let mut blocks = Vec::new();
                        let mut text_lines = Vec::new();
                        for c in a.content {
                            match c {
                                HistoricalContent::Text { text } => {
                                    text_lines.push(ratatui::text::Line::from(text));
                                }
                                HistoricalContent::Thinking { thinking } => {
                                    if !text_lines.is_empty() {
                                        blocks.push(MessageBlock::Text(std::mem::take(&mut text_lines)));
                                    }
                                    blocks.push(MessageBlock::Thinking(ThinkingBlock {
                                        thinking_text: thinking,
                                        is_expanded: false,
                                        is_redacted: false,
                                    }));
                                }
                                HistoricalContent::ToolCall { id, name, arguments } => {
                                    if !text_lines.is_empty() {
                                        blocks.push(MessageBlock::Text(std::mem::take(&mut text_lines)));
                                    }
                                    let json_text = serde_json::to_string_pretty(&arguments).unwrap_or_default();
                                    blocks.push(MessageBlock::ToolCall(ToolCallWidget {
                                        call_id: id,
                                        name,
                                        state: ToolCallState::Pending,
                                        content: json_text.lines().map(|l| ratatui::text::Line::from(l.to_string())).collect(),
                                        is_expanded: false,
                                    }));
                                }
                            }
                        }
                        if !text_lines.is_empty() {
                            blocks.push(MessageBlock::Text(text_lines));
                        }
                        (MessageRole::Assistant, blocks)
                    }
                    HistoricalMessage::ToolResult(t) => {
                        let text = t.content.into_iter().filter_map(|c| {
                            match c {
                                HistoricalContent::Text { text } => Some(text),
                                _ => None,
                            }
                        }).collect::<Vec<_>>().join("\n");
                        let display = format!("ToolResult({}): {}", t.tool_name, text);
                        (MessageRole::Assistant, vec![MessageBlock::Text(vec![ratatui::text::Line::from(display)])])
                    }
                };
                RenderedMessage {
                    role,
                    blocks,
                    timestamp: std::time::SystemTime::now(),
                    status: MessageStatus::Complete,
                }
            })
            .collect()
    }

    fn open_external_editor(&mut self) {
        let editor_cmd = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "vi".to_string());

        let current_text = self.editor.lines.join("\n");
        let temp_file = temp_dir_for_pandaria().join(format!("pandaria_edit_{}.md", std::process::id()));

        if let Err(e) = std::fs::write(&temp_file, &current_text) {
            self.data.last_error = Some(format!("Failed to write temp file: {}", e));
            return;
        }

        // Suspend raw mode and run editor
        let _ = crossterm::terminal::disable_raw_mode();
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{} '{}'", editor_cmd, temp_file.display()))
            .status();
        let _ = crossterm::terminal::enable_raw_mode();

        match status {
            Ok(s) if s.success() => {
                match std::fs::read_to_string(&temp_file) {
                    Ok(content) => {
                        self.editor.clear();
                        self.editor.insert_text(content.trim_end());
                    }
                    Err(e) => {
                        self.data.last_error = Some(format!("Failed to read temp file: {}", e));
                    }
                }
            }
            Ok(s) => {
                self.data.last_error = Some(format!("Editor exited with status: {}", s));
            }
            Err(e) => {
                self.data.last_error = Some(format!("Failed to launch editor: {}", e));
            }
        }

        let _ = std::fs::remove_file(&temp_file);
    }

    fn open_model_selector(&mut self) {
        let models = self.config.ui.models.clone();
        self.overlays.push(Box::new(
            crate::overlays::model_selector::ModelSelector::new(models),
        ));
    }

    fn cycle_model(&mut self, forward: bool) {
        let models: Vec<&str> = self.config.ui.models.iter().map(|s| s.as_str()).collect();
        let current = self.data.active_session().info.model.as_str();
        let pos = models.iter().position(|&m| m == current).unwrap_or(0);
        let new_pos = if forward {
            (pos + 1) % models.len()
        } else {
            pos.checked_sub(1).unwrap_or(models.len() - 1) % models.len()
        };
        let new_model = models[new_pos].to_string();

        let rest = self.rest.clone();
        let token = self.config.auth.token.clone().unwrap_or_default();
        let sid = self.data.active_session().info.id.clone();
        let task_tx = self.task_tx.clone();
        let new_model_clone = new_model.clone();
        tokio::spawn(async move {
            match rest.update_model(&sid, &new_model_clone, &token).await {
                Ok(info) => {
                    let _ = task_tx.send(TaskAction::SessionFetched(info)).await;
                }
                Err(e) => tracing::warn!("update model failed: {e}"),
            }
        });

        self.data.active_session_mut().info.model = new_model;
    }

    pub fn render_ui(&mut self, f: &mut Frame) {
        let theme = &self.theme;
        let session = self.data.active_session();

        // Update stateful components from session data
        // (These are done inline in render rather than stored, to avoid borrow issues)

        let pending = self.input_queue.pending_texts();
        let pending_height = if pending.is_empty() {
            0
        } else {
            (pending.len().min(2) + 1).min(3) as u16
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(pending_height),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
            .split(f.area());

        // HeaderBar
        self.header.update(session.info.id.clone(), session.info.model.clone());
        self.header.render(chunks[0], f.buffer_mut());

        // SessionTabs
        let tabs_data: Vec<(String, String)> = self
            .data
            .sessions
            .keys()
            .map(|id| (id.clone(), id.chars().take(8).collect()))
            .collect();
        self.session_tabs.update(tabs_data, self.data.active_session.clone());
        self.session_tabs.render(chunks[1], f.buffer_mut());

        // ChatView
        render_chat(f, chunks[2], theme, session);

        // PendingMessagesWidget
        if !pending.is_empty() {
            PendingMessagesWidget::new(&pending, theme).render(chunks[3], f.buffer_mut());
        }

        // StatusBar
        render_status_bar(
            chunks[4],
            f.buffer_mut(),
            theme,
            &self.data.connection_status,
            self.state == AppState::Busy,
            &self.spinner,
            self.input_tokens,
            self.context_window,
            &session.info.model,
            self.input_queue.len(),
            self.input_queue.strategy(),
        );

        // Editor
        self.editor.render(chunks[5], f.buffer_mut());

        // Render overlays on top
        let full_area = f.area();
        for overlay in &self.overlays {
            overlay.render(full_area, f.buffer_mut());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::model::{SessionInfo, UsageInfo};
    use crate::config::{AuthConfig, Config, ServerConfig, UiConfig};
    use crate::state::{MessageBlock, MessageRole, MessageStatus, RenderedMessage, StreamingBuffer, ToolCallState};

    fn make_test_config() -> Config {
        Config {
            server: ServerConfig {
                url: "http://localhost:9999".to_string(),
                timeout_secs: 1,
            },
            auth: AuthConfig {
                token: Some("test-token".to_string()),
            },
            ui: UiConfig {
                max_history: 10,
                show_tool_calls: true,
                syntax_theme: "base16-ocean.dark".to_string(),
                scrollback: 100,
                models: crate::config::default_models(),
            },
            keys: None,
        }
    }

    fn make_session_info(id: &str) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            title: None,
            model: "gpt-4o".to_string(),
            context_window: Some(200_000),
            created_at: None,
        }
    }

    fn make_test_app() -> App {
        let config = make_test_config();
        let info = make_session_info("s1");
        App::new(config, "s1".to_string(), info)
    }

    fn make_app_with_streaming() -> App {
        let mut app = make_test_app();
        app.state = AppState::Busy;
        let assistant_msg = RenderedMessage {
            role: MessageRole::Assistant,
            blocks: Vec::new(),
            timestamp: std::time::SystemTime::now(),
            status: MessageStatus::Streaming,
        };
        app.data.active_session_mut().messages.push(assistant_msg);
        app.data.active_session_mut().streaming = Some(StreamingBuffer {
            text_content: String::new(),
            thinking_content: String::new(),
            pending_tool_calls: Vec::new(),
            tool_arg_buffers: std::collections::HashMap::new(),
        });
        app
    }

    #[test]
    fn test_handle_text_delta_appends_to_streaming_message() {
        let mut app = make_app_with_streaming();
        app.handle_server_event(ServerEvent::TextDelta {
            delta: "hello".to_string(),
        });
        let session = app.data.active_session();
        assert_eq!(session.streaming.as_ref().unwrap().text_content, "hello");
        let last = session.messages.last().unwrap();
        assert!(matches!(last.blocks.last(), Some(MessageBlock::Text(_))));
    }

    #[test]
    fn test_handle_thinking_delta_creates_thinking_block() {
        let mut app = make_app_with_streaming();
        app.handle_server_event(ServerEvent::ThinkingDelta {
            content_index: 0,
            delta: "thinking...".to_string(),
        });
        let session = app.data.active_session();
        assert_eq!(
            session.streaming.as_ref().unwrap().thinking_content,
            "thinking..."
        );
        let last = session.messages.last().unwrap();
        assert!(last
            .blocks
            .iter()
            .any(|b| matches!(b, MessageBlock::Thinking(_))));
    }

    #[test]
    fn test_handle_tool_call_started_adds_pending_block() {
        let mut app = make_app_with_streaming();
        app.handle_server_event(ServerEvent::ToolCallStarted {
            call_id: "c1".to_string(),
            name: "read".to_string(),
        });
        let session = app.data.active_session();
        let last = session.messages.last().unwrap();
        let tc = last
            .blocks
            .iter()
            .find_map(|b| match b {
                MessageBlock::ToolCall(tc) => Some(tc),
                _ => None,
            })
            .expect("tool call block");
        assert_eq!(tc.call_id, "c1");
        assert_eq!(tc.name, "read");
        assert_eq!(tc.state, ToolCallState::Pending);
    }

    #[test]
    fn test_handle_tool_call_done_updates_state_to_success() {
        let mut app = make_app_with_streaming();
        app.handle_server_event(ServerEvent::ToolCallStarted {
            call_id: "c1".to_string(),
            name: "read".to_string(),
        });
        app.handle_server_event(ServerEvent::ToolCallDone {
            call_id: "c1".to_string(),
            result: Some("content".to_string()),
            is_error: false,
        });
        let session = app.data.active_session();
        let last = session.messages.last().unwrap();
        let tc = last
            .blocks
            .iter()
            .find_map(|b| match b {
                MessageBlock::ToolCall(tc) => Some(tc),
                _ => None,
            })
            .expect("tool call block");
        assert_eq!(tc.state, ToolCallState::Success);
        assert!(tc.content.iter().any(|l| l
            .spans
            .iter()
            .any(|s| s.content == "content")));
    }

    #[test]
    fn test_handle_tool_call_done_updates_state_to_error() {
        let mut app = make_app_with_streaming();
        app.handle_server_event(ServerEvent::ToolCallStarted {
            call_id: "c1".to_string(),
            name: "read".to_string(),
        });
        app.handle_server_event(ServerEvent::ToolCallDone {
            call_id: "c1".to_string(),
            result: Some("err".to_string()),
            is_error: true,
        });
        let session = app.data.active_session();
        let last = session.messages.last().unwrap();
        let tc = last
            .blocks
            .iter()
            .find_map(|b| match b {
                MessageBlock::ToolCall(tc) => Some(tc),
                _ => None,
            })
            .expect("tool call block");
        assert_eq!(tc.state, ToolCallState::Error);
    }

    #[test]
    fn test_handle_turn_end_sets_complete_status() {
        let mut app = make_app_with_streaming();
        app.handle_server_event(ServerEvent::TurnEnd {
            stop_reason: "stop".to_string(),
            usage: None,
        });
        let session = app.data.active_session();
        assert!(session.streaming.is_none());
        assert_eq!(session.messages.last().unwrap().status, MessageStatus::Complete);
        assert_eq!(app.state, AppState::Connected);
    }

    #[test]
    fn test_handle_turn_end_updates_input_tokens() {
        let mut app = make_app_with_streaming();
        app.handle_server_event(ServerEvent::TurnEnd {
            stop_reason: "stop".to_string(),
            usage: Some(UsageInfo {
                input_tokens: 42,
                output_tokens: 10,
            }),
        });
        assert_eq!(app.input_tokens, 42);
    }

    #[test]
    fn test_handle_error_sets_message_status_to_error() {
        let mut app = make_app_with_streaming();
        app.handle_server_event(ServerEvent::Error {
            code: "err".to_string(),
            message: "bad".to_string(),
        });
        let session = app.data.active_session();
        assert_eq!(session.messages.last().unwrap().status, MessageStatus::Error);
        assert!(session.error.is_some());
        assert_eq!(app.state, AppState::Connected);
    }

    #[test]
    fn test_handle_task_action_session_created() {
        let mut app = make_test_app();
        let info = SessionInfo {
            id: "s2".to_string(),
            title: Some("new".to_string()),
            model: "claude".to_string(),
            context_window: Some(100_000),
            created_at: None,
        };
        app.handle_task_action(TaskAction::SessionCreated(info));
        assert!(app.data.sessions.contains_key("s2"));
        assert_eq!(app.data.active_session, "s2");
        assert_eq!(app.context_window, Some(100_000));
    }

    #[test]
    fn test_handle_task_action_connection_tested_ok() {
        let mut app = make_test_app();
        app.data.last_error = Some("old".to_string());
        app.handle_task_action(TaskAction::ConnectionTested {
            url: "http://x".to_string(),
            ok: true,
        });
        // ok=true only logs, does not clear last_error
        assert_eq!(app.data.last_error, Some("old".to_string()));
    }

    #[test]
    fn test_handle_task_action_connection_tested_fail() {
        let mut app = make_test_app();
        app.handle_task_action(TaskAction::ConnectionTested {
            url: "http://x".to_string(),
            ok: false,
        });
        assert_eq!(
            app.data.last_error,
            Some("Could not connect to http://x".to_string())
        );
    }

    #[test]
    fn test_handle_task_action_session_fetched_updates_info() {
        let mut app = make_test_app();
        let info = SessionInfo {
            id: "s1".to_string(),
            title: Some("updated".to_string()),
            model: "claude".to_string(),
            context_window: Some(50_000),
            created_at: None,
        };
        app.handle_task_action(TaskAction::SessionFetched(info));
        let session = app.data.sessions.get("s1").expect("session s1");
        assert_eq!(session.info.title, Some("updated".to_string()));
        assert_eq!(app.context_window, Some(50_000));
    }
}
