use crate::component::{Component, InputResult, OverlayResult};
use crate::ui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Widget};

pub struct ModelSelector {
    models: Vec<String>,
    selected: usize,
    confirmed: Option<String>,
    dismissed: bool,
}

impl ModelSelector {
    pub fn new(models: Vec<String>) -> Self { Self { models, selected: 0, confirmed: None, dismissed: false } }
}

impl Component for ModelSelector {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let theme = Theme::default();
        let items: Vec<ListItem> = self.models.iter().enumerate().map(|(i, m)| {
            if i == self.selected { ListItem::new(Span::styled(format!("▶ {}", m), Style::default().fg(theme.accent).add_modifier(Modifier::BOLD))) }
            else { ListItem::new(Span::styled(format!("  {}", m), Style::default().fg(theme.text))) }
        }).collect();
        let block = Block::default().borders(Borders::ALL).title("Select Model").style(Style::default().fg(theme.text));
        let overlay_area = centered_rect(40, (items.len() as u16 + 2).min(12), area);
        Clear.render(overlay_area, buf);
        List::new(items).block(block).render(overlay_area, buf);
    }

    fn handle_input(&mut self, key: KeyEvent) -> InputResult {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => { if self.selected > 0 { self.selected -= 1; } InputResult::Consumed }
            KeyCode::Down | KeyCode::Char('j') => { if self.selected + 1 < self.models.len() { self.selected += 1; } InputResult::Consumed }
            KeyCode::Enter => { self.confirmed = self.models.get(self.selected).cloned(); InputResult::Consumed }
            KeyCode::Esc => { self.dismissed = true; InputResult::Consumed }
            _ => InputResult::Consumed,
        }
    }

    fn take_result(&mut self) -> OverlayResult {
        if self.dismissed { return OverlayResult::Dismissed; }
        self.confirmed.take().map_or(OverlayResult::Pending, OverlayResult::Confirmed)
    }
}

fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    Rect::new(r.x + (r.width.saturating_sub(width)) / 2, r.y + (r.height.saturating_sub(height)) / 2, width.min(r.width), height.min(r.height))
}
