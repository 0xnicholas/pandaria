use crate::client::model::ServerEvent;
use crate::client::rest::RestClient;
use crate::client::sse;
use crate::command::Command;
use crate::config::Config;
use crate::overlays::{OverlayAction, OverlayStack};
use crate::paste::PasteStore;
use crate::state::*;
use crate::ui::theme::Theme;
use crate::widgets::chat_view::ChatView;
use crate::widgets::header::HeaderBar;
use crate::widgets::input_bar::InputBar;
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
    pub input: InputBar,
    pub spinner: SpinnerWidget,
    pub overlays: OverlayStack,
    pub reqwest_client: reqwest::Client,
    pub paste_store: PasteStore,
    pub context_window: Option<u64>,
    pub input_tokens: u64,
    pub server_rx: Option<mpsc::Receiver<ServerEvent>>,
    pub scroll_offset: usize,
    pub running: bool,
}

impl App {
    pub fn new(config: Config, session_id: String, session_info: crate::client::model::SessionInfo) -> Self {
        let rest = RestClient::new(&config.server);
        let data = crate::state::State::new(session_id, session_info);
        let context_window = data.active_session().info.context_window;
        Self {
            state: AppState::Connected,
            data,
            config,
            theme: Theme::default(),
            rest,
            input: InputBar::new(),
            spinner: SpinnerWidget::new(),
            overlays: OverlayStack::new(),
            reqwest_client: reqwest::Client::new(),
            paste_store: PasteStore::new(),
            context_window,
            input_tokens: 0,
            server_rx: None,
            scroll_offset: 0,
            running: true,
        }
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        // Overlays take priority
        if !self.overlays.is_empty() {
            if let Some(overlay) = self.overlays.top_mut() {
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

        match key.code {
            KeyCode::Enter => self.submit_input(),
            KeyCode::Esc => {
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
                    self.input.clear();
                }
            }
            KeyCode::Up => {
                if self.input.buffer.is_empty() {
                    self.input.history_prev();
                }
            }
            KeyCode::Down => {
                if self.input.buffer.is_empty() {
                    self.input.history_next();
                }
            }
            KeyCode::Backspace => self.input.delete_backward(),
            KeyCode::Left => self.input.move_cursor_left(),
            KeyCode::Right => self.input.move_cursor_right(),
            KeyCode::Home => self.input.move_cursor_home(),
            KeyCode::End => self.input.move_cursor_end(),
            KeyCode::Char(c) => {
                if key.modifiers == KeyModifiers::CONTROL {
                    match c {
                        'c' => {
                            if self.state == AppState::Busy {
                                let rest = RestClient::new(&self.config.server);
                                let token = self.config.auth.token.clone().unwrap_or_default();
                                let sid = self.data.active_session.clone();
                                tokio::spawn(async move {
                                    let _ = rest.interrupt(&sid, &token).await;
                                });
                                self.state = AppState::Connected;
                            } else {
                                self.running = false;
                            }
                        }
                        'd' if self.input.buffer.is_empty() => self.running = false,
                        'o' => {
                            for msg in &mut self.data.active_session_mut().messages {
                                for block in &mut msg.blocks {
                                    if let MessageBlock::ToolCall(tc) = block {
                                        tc.toggle();
                                    }
                                }
                            }
                        }
                        't' => {
                            for msg in &mut self.data.active_session_mut().messages {
                                for block in &mut msg.blocks {
                                    if let MessageBlock::Thinking(tb) = block {
                                        tb.toggle();
                                    }
                                }
                            }
                        }
                        'l' => {
                            let models =
                                vec!["gpt-4o".to_string(), "claude-sonnet-4".to_string()];
                            self.overlays.push(Box::new(
                                crate::overlays::model_selector::ModelSelector::new(models),
                            ));
                        }
                        's' => {
                            let sessions: Vec<_> = self
                                .data
                                .sessions
                                .iter()
                                .map(|(id, state)| {
                                    (
                                        id.clone(),
                                        state
                                            .info
                                            .title
                                            .clone()
                                            .unwrap_or_else(|| id.chars().take(8).collect()),
                                    )
                                })
                                .collect();
                            self.overlays.push(Box::new(
                                crate::overlays::session_list::SessionListOverlay::new(
                                    sessions,
                                ),
                            ));
                        }
                        _ => {}
                    }
                } else {
                    match c {
                        '/' if self.input.buffer.is_empty() => {
                            self.overlays.push(Box::new(
                                crate::overlays::command_palette::CommandPalette::new(),
                            ));
                        }
                        _ => self.input.insert_char(c),
                    }
                }
            }
            _ => {}
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
                    let rest = RestClient::new(&self.config.server);
                    let token = self.config.auth.token.clone().unwrap_or_default();
                    tokio::spawn(async move {
                        let _ = rest.create_session(title.as_deref(), &token).await;
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
            let _ = sse::connect(&reqwest_client, &base_url, &sid_clone, &token_clone, None, tx).await;
        });

        tokio::spawn(async move {
            let _ = rest.send_message(&sid, &content, &token).await;
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
    }

    pub fn handle_server_event(&mut self, event: ServerEvent) {
        let session = self.data.active_session_mut();
        match event {
            ServerEvent::MessageStart { .. } => {}
            ServerEvent::TextDelta { delta } => {
                if let Some(ref mut buf) = session.streaming {
                    buf.text_content.push_str(&delta);
                }
                if let Some(last) = session.messages.last_mut() {
                    if let Some(ref buf) = session.streaming {
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
                }
                self.scroll_offset = 0;
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
                self.scroll_offset = 0;
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
                self.scroll_offset = 0;
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
                        if let MessageBlock::ToolCall(tc) = block {
                            if tc.call_id == call_id {
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
                Constraint::Length(1),
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

        self.input.render(
            f,
            chunks[4],
            theme,
            self.state == AppState::Busy,
            true,
        );
    }
}
