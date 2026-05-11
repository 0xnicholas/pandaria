use crate::autocomplete::Suggestion;
use crate::overlays::{Overlay, OverlayAction};
use crate::ui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem},
    Frame,
};

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
    fn render(&self, f: &mut Frame, area: Rect) {
        let theme = Theme::default();
        let max_height = 8u16;
        let width = 50u16;
        let height = (self.suggestions.len() as u16 + 2).min(max_height).max(3);

        let overlay_area = Rect::new(
            area.x + 2,
            area.y.saturating_sub(height),
            width.min(area.width.saturating_sub(4)),
            height,
        );

        let items: Vec<ListItem> = self.suggestions.iter().enumerate().map(|(i, s)| {
            let style = if i == self.selected {
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            let desc = s.description.as_ref()
                .map(|d| Span::styled(format!(" — {}", d), Style::default().fg(theme.muted)))
                .unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::styled(&s.label, style),
                desc,
            ]))
        }).collect();

        let block = Block::default()
            .borders(Borders::ALL)
            .title("Suggestions")
            .style(Style::default().fg(theme.text));
        let list = List::new(items).block(block);
        f.render_widget(Clear, overlay_area);
        f.render_widget(list, overlay_area);
    }

    fn handle_input(&mut self, key: KeyEvent) -> OverlayAction {
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

    fn is_capturing(&self) -> bool { true }
}
