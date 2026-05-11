use crate::autocomplete::{
    AutocompleteContext, AutocompleteProvider, FilePathProvider, SlashCommandProvider,
};
use crate::client::model::ServerEvent;
use crate::client::rest::RestClient;
use crate::client::sse;
use crate::command::Command;
use crate::config::Config;
use crate::keybindings::{Keybinding, KeybindingsManager};
use crate::overlays::{OverlayAction, OverlayStack};
use crate::paste::PasteStore;
use crate::state::*;
use crate::ui::theme::Theme;
use crate::widgets::chat_view::ChatView;
use crate::widgets::editor::Editor;
use crate::widgets::header::HeaderBar;
use crate::widgets::session_tabs::SessionTabsWidget;
use crate::widgets::spinner::SpinnerWidget;
use crate::widgets::status_bar::StatusBar;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::Frame;
use std::collections::HashMap;
use tokio::sync::mpsc;

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
    pub overlays: OverlayStack,
    pub reqwest_client: reqwest::Client,
    pub paste_store: PasteStore,
    pub context_window: Option<u64>,
    pub input_tokens: u64,
    pub server_rx: Option<mpsc::Receiver<ServerEvent>>,
    pub scroll_offset: usize,
    pub user_scrolled_up: bool,
    pub running: bool,
}

impl App {
    pub fn new(config: Config, session_id: String, session_info: crate::client::model::SessionInfo) -> Self {
        let rest = RestClient::new(&config.server);
        let data = crate::state::State::new(session_id, session_info);
        let context_window = data.active_session().info.context_window;
        let mut keybindings = KeybindingsManager::new();
        if let Some(ref keys_config) = config.keys {
            keybindings.load_user_config(keys_config);
        }
        Self {
            state: AppState::Connected,
            data,
            config,
            theme: Theme::default(),
            rest,
            editor: Editor::new(),
            keybindings,
            autocomplete_providers: vec![
                Box::new(SlashCommandProvider::new()),
                Box::new(FilePathProvider::new()),
            ],
            spinner: SpinnerWidget::new(),
            overlays: OverlayStack::new(),
            reqwest_client: reqwest::Client::new(),
            paste_store: PasteStore::new(),
            context_window,
            input_tokens: 0,
            server_rx: None,
            scroll_offset: 0,
            user_scrolled_up: false,
            running: true,
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
        if !self.overlays.is_empty() {
            if let Some(overlay) = self.overlays.top_mut() {
                // Non-capturing overlays dismiss on any printable input
                if !overlay.is_capturing() {
                    match key.code {
                        KeyCode::Char(_) | KeyCode::Enter => {
                            self.overlays.pop();
                            return;
                        }
                        _ => {}
                    }
                }
                let action = overlay.handle_input(key);
                match action {
                    OverlayAction::Dismiss => {
                        self.overlays.pop();
                    }
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

        // Autocomplete overlay logic
        let ctx = self.build_autocomplete_context();
        for provider in &self.autocomplete_providers {
            if provider.should_trigger(&ctx) {
                let suggestions = provider.get_suggestions(&ctx);
                if !suggestions.is_empty() {
                    self.overlays.push(Box::new(
                        crate::overlays::autocomplete::AutocompleteOverlay::new(suggestions),
                    ));
                    return;
                }
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
            let models = vec!["gpt-4o".to_string(), "claude-sonnet-4-20250514".to_string()];
            self.overlays.push(Box::new(
                crate::overlays::model_selector::ModelSelector::new(models),
            ));
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

        // --- Editor keybindings ---
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
        if kb.matches(&key, Keybinding::AutocompleteTrigger) {
            // Already handled by the autocomplete_providers loop above
            return;
        }
        if kb.matches(&key, Keybinding::AppOpenCommandPalette) {
            if self.editor.is_empty() {
                self.overlays.push(Box::new(
                    crate::overlays::command_palette::CommandPalette::new(),
                ));
            }
            return;
        }

        // Char input (only when no modifier matches caught it)
        if let KeyCode::Char(ch) = key.code {
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                self.editor.insert_char(ch);
            }
        }
    }

    fn handle_overlay_confirm(&mut self, value: String) {
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
                    let title_clone = title.clone();
                    tokio::spawn(async move {
                        match rest.create_session(title_clone.as_deref(), &token).await {
                            Ok(info) => tracing::info!(session_id = %info.id, "created new session"),
                            Err(e) => tracing::error!("create session failed: {e}"),
                        }
                    });
                }
                Command::SwitchSession { id } => {
                    if self.data.sessions.contains_key(&id) {
                        self.data.active_session = id;
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
                        let session = self.data.active_session_mut();
                        session.info.model = model_id;
                    } else {
                        let models =
                            vec!["gpt-4o".to_string(), "claude-sonnet-4".to_string()];
                        self.overlays.push(Box::new(
                            crate::overlays::model_selector::ModelSelector::new(models),
                        ));
                    }
                }
                Command::Connect { url } => {
                    self.config.server.url = url;
                }
                Command::Auth { token } => {
                    self.config.auth.token = Some(token);
                }
                Command::Tokens => { /* Token info displayed in StatusBar gauge */ }
            }
        }
    }

    pub fn handle_paste(&mut self, data: String) {
        let result = self.paste_store.store(&data);
        self.editor.insert_text(&result);
    }

    fn submit_input(&mut self) {
        let text = self.editor.take_text();
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
            blocks: vec![MessageBlock::Text(vec![ratatui::text::Line::from(
                text.clone(),
            )])],
            timestamp: std::time::SystemTime::now(),
            status: MessageStatus::Complete,
        };
        self.data.active_session_mut().messages.push(msg);

        let rest = RestClient::new(&self.config.server);
        let token = self.config.auth.token.clone().unwrap_or_default();
        let sid = self.data.active_session.clone();
        let content = text.clone();

        let (tx, rx) = mpsc::channel::<ServerEvent>(32);
        let reqwest_client = reqwest::Client::new();
        let base_url = self.config.server.url.clone();

        let sid_clone = sid.clone();
        let token_clone = token.clone();
        tokio::spawn(async move {
            if let Err(e) = sse::connect(&reqwest_client, &base_url, &sid_clone, &token_clone, None, tx).await {
                tracing::error!("SSE connect failed: {e}");
            }
        });

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

    pub fn handle_server_event(&mut self, event: ServerEvent) {
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

    pub fn render_ui(&self, f: &mut Frame) {
        let theme = &self.theme;
        let session = self.data.active_session();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
            .split(f.area());

        HeaderBar::render(f, chunks[0], theme, &session.info.id, &session.info.model);

        let tabs_data: Vec<(String, String)> = self
            .data
            .sessions
            .keys()
            .map(|id| (id.clone(), id.chars().take(8).collect()))
            .collect();
        SessionTabsWidget::render(f, chunks[1], theme, &tabs_data, &self.data.active_session);

        ChatView::render(f, chunks[2], theme, session);

        StatusBar::render(
            f,
            chunks[3],
            theme,
            &self.data.connection_status,
            self.state == AppState::Busy,
            &self.spinner,
            self.input_tokens,
            self.context_window,
            &session.info.model,
        );

        self.editor.render(
            f,
            chunks[4],
            theme,
            self.state == AppState::Busy,
            true,
        );
    }
}
