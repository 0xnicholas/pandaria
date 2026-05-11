use crate::overlays::{Overlay, OverlayAction};
use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub struct HelpOverlay { lines: Vec<Line<'static>> }

impl Default for HelpOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl HelpOverlay {
    pub fn new() -> Self {
        let text = [
            ("Keybindings", true), ("", false),
            ("Enter               Submit message", false), ("Shift+Enter / Alt+Enter  New line", false),
            ("Esc                 Cancel / interrupt", false),
            ("Ctrl+C              Quit (when idle)", false),
            ("Ctrl+O              Toggle tool calls", false), ("Ctrl+T              Toggle thinking blocks", false),
            ("Ctrl+L              Select model", false), ("Ctrl+S              List sessions", false),
            ("Ctrl+A / Home       Line start", false), ("Ctrl+E / End        Line end", false),
            ("Ctrl+B / Left       Cursor left", false), ("Ctrl+F / Right      Cursor right", false),
            ("Alt+Left            Word left", false), ("Alt+Right           Word right", false),
            ("Ctrl+W              Delete word back", false), ("Alt+D               Delete word forward", false),
            ("Ctrl+U              Delete to line start", false), ("Ctrl+K              Delete to line end", false),
            ("Ctrl+Y              Yank (paste)", false), ("Alt+Y               Yank pop (cycle)", false),
            ("Ctrl+-              Undo", false),
            ("Tab                 Trigger autocomplete", false),
            ("PgUp / PgDn         Page up/down", false),
            ("", false), ("Commands", true), ("", false),
            ("/quit /q            Quit", false), ("/new [title]        New session", false),
            ("/switch <id>        Switch session", false), ("/list               List sessions", false),
            ("/model [id]         Select model", false), ("/clear              Clear view", false),
            ("/connect <url>      Connect to server", false), ("/auth <token>       Set auth token", false),
            ("/tokens             View usage", false), ("/help               Show this help", false),
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
