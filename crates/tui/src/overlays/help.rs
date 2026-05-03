use crate::overlays::{Overlay, OverlayAction};
use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub struct HelpOverlay { lines: Vec<Line<'static>> }

impl HelpOverlay {
    pub fn new() -> Self {
        let text = [
            ("Keybindings", true), ("", false),
            ("Enter        Submit message", false), ("Esc          Cancel / interrupt", false),
            ("Ctrl+C       Quit", false), ("Ctrl+D       Quit when input empty", false),
            ("Ctrl+O       Toggle tool calls", false), ("Ctrl+T       Toggle thinking blocks", false),
            ("Ctrl+L       Select model", false), ("Ctrl+S       Session list", false),
            ("", false), ("Commands", true), ("", false),
            ("/quit /q          Quit", false), ("/new [title]      New session", false),
            ("/switch <id>      Switch session", false), ("/list             List sessions", false),
            ("/model [id]       Select model", false), ("/clear            Clear view", false),
            ("/connect <url>    Connect to server", false), ("/auth <token>     Set auth token", false),
            ("/tokens           View usage", false), ("/help             Show this help", false),
        ];
        Self { lines: text.iter().map(|(t, b)| if *b { Line::from(Span::styled(*t, Style::default().add_modifier(Modifier::BOLD))) } else { Line::from(*t) }).collect() }
    }
}

impl Overlay for HelpOverlay {
    fn render(&self, f: &mut Frame, _area: Rect) {
        let theme = Theme::default();
        let block = Block::default().borders(Borders::ALL).title("Help").style(Style::default().fg(theme.text));
        let inner = block.inner(f.area());
        f.render_widget(Clear, f.area());
        f.render_widget(Paragraph::new(self.lines.clone()).block(block), inner);
    }
    fn handle_input(&mut self, _key: crossterm::event::KeyEvent) -> OverlayAction { OverlayAction::Dismiss }
    fn is_capturing(&self) -> bool { false }
}
