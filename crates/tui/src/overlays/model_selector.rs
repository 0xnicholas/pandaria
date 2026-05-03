use crate::overlays::{Overlay, OverlayAction};
use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem};
use ratatui::Frame;

pub struct ModelSelector { models: Vec<String>, selected: usize }

impl ModelSelector {
    pub fn new(models: Vec<String>) -> Self { Self { models, selected: 0 } }
    pub fn selected_model(&self) -> Option<&str> { self.models.get(self.selected).map(|s| s.as_str()) }
}

impl Overlay for ModelSelector {
    fn render(&self, f: &mut Frame, _area: Rect) {
        let theme = Theme::default();
        let items: Vec<ListItem> = self.models.iter().enumerate().map(|(i, m)| {
            if i == self.selected { ListItem::new(Span::styled(format!("▶ {}", m), Style::default().fg(theme.accent).add_modifier(Modifier::BOLD))) }
            else { ListItem::new(Span::styled(format!("  {}", m), Style::default().fg(theme.text))) }
        }).collect();
        let block = Block::default().borders(Borders::ALL).title("Select Model").style(Style::default().fg(theme.text));
        let area = centered_rect(40, (items.len() as u16 + 2).min(12), f.area());
        f.render_widget(Clear, area);
        f.render_widget(List::new(items).block(block), area);
    }
    fn handle_input(&mut self, key: crossterm::event::KeyEvent) -> OverlayAction {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => { if self.selected > 0 { self.selected -= 1; } OverlayAction::Consumed }
            KeyCode::Down | KeyCode::Char('j') => { if self.selected + 1 < self.models.len() { self.selected += 1; } OverlayAction::Consumed }
            KeyCode::Enter => OverlayAction::Confirm(self.selected_model().unwrap_or("").to_string()),
            KeyCode::Esc => OverlayAction::Dismiss,
            _ => OverlayAction::Consumed,
        }
    }
}

fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    Rect::new(r.x + (r.width.saturating_sub(width)) / 2, r.y + (r.height.saturating_sub(height)) / 2, width.min(r.width), height.min(r.height))
}
