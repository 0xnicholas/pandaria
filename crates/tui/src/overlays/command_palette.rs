use crate::overlays::{Overlay, OverlayAction};
use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem};
use ratatui::Frame;

const COMMANDS: &[(&str, &str)] = &[
    ("/quit", "Quit"), ("/new", "New session"), ("/switch <id>", "Switch session"),
    ("/list", "List sessions"), ("/model <id>", "Select model"), ("/clear", "Clear view"),
    ("/connect <url>", "Connect"), ("/auth <token>", "Set token"), ("/tokens", "Show usage"), ("/help", "Help"),
];

pub struct CommandPalette { filter: String, selected: usize }

impl Default for CommandPalette {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandPalette {
    pub fn new() -> Self { Self { filter: String::new(), selected: 0 } }
    fn filtered(&self) -> Vec<(&'static str, &'static str)> {
        if self.filter.is_empty() { COMMANDS.to_vec() }
        else { COMMANDS.iter().filter(|(c, d)| c.contains(&self.filter) || d.to_lowercase().contains(&self.filter.to_lowercase())).copied().collect() }
    }
}

impl Overlay for CommandPalette {
    fn render(&self, f: &mut Frame, _area: Rect) {
        let theme = Theme::default();
        let filtered = self.filtered();
        let items: Vec<ListItem> = filtered.iter().enumerate().map(|(i, (cmd, desc))| {
            if i == self.selected { ListItem::new(Line::from(vec![Span::styled(*cmd, Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)), Span::styled(format!(" — {}", desc), Style::default().fg(theme.muted))])) }
            else { ListItem::new(Line::from(vec![Span::styled(*cmd, Style::default().fg(theme.text)), Span::styled(format!(" — {}", desc), Style::default().fg(theme.muted))])) }
        }).collect();
        let block = Block::default().borders(Borders::ALL).title(format!("Commands ({})", filtered.len())).style(Style::default().fg(theme.text));
        let h = (filtered.len() as u16 + 2).min(12);
        let area = Rect::new(f.area().x + (f.area().width.saturating_sub(50)) / 2, f.area().y + (f.area().height.saturating_sub(h)) / 3, 50, h);
        f.render_widget(Clear, area);
        f.render_widget(List::new(items).block(block), area);
    }
    fn handle_input(&mut self, key: crossterm::event::KeyEvent) -> OverlayAction {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Char(c) => { self.filter.push(c); self.selected = 0; OverlayAction::Consumed }
            KeyCode::Backspace => { self.filter.pop(); self.selected = 0; OverlayAction::Consumed }
            KeyCode::Up => { if self.selected > 0 { self.selected -= 1; } OverlayAction::Consumed }
            KeyCode::Down => { let n = self.filtered().len(); if n > 0 && self.selected + 1 < n { self.selected += 1; } OverlayAction::Consumed }
            KeyCode::Enter => {
                let f = self.filtered();
                if let Some((cmd, _)) = f.get(self.selected) { OverlayAction::Confirm(cmd.to_string()) }
                else { OverlayAction::Dismiss }
            }
            KeyCode::Esc => OverlayAction::Dismiss,
            _ => OverlayAction::Consumed,
        }
    }
}
