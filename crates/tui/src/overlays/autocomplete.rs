use crate::autocomplete::Suggestion;
use crate::component::{Component, InputResult, OverlayResult};
use crate::ui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Widget};

pub struct AutocompleteOverlay {
    suggestions: Vec<Suggestion>,
    selected: usize,
    confirmed: Option<String>,
    dismissed: bool,
}

impl AutocompleteOverlay {
    pub fn new(suggestions: Vec<Suggestion>) -> Self {
        Self {
            suggestions,
            selected: 0,
            confirmed: None,
            dismissed: false,
        }
    }
}

impl Component for AutocompleteOverlay {
    fn render(&self, area: Rect, buf: &mut Buffer) {
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

        let items: Vec<ListItem> = self
            .suggestions
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let style = if i == self.selected {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text)
                };
                let desc = s
                    .description
                    .as_ref()
                    .map(|d| Span::styled(format!(" — {}", d), Style::default().fg(theme.muted)))
                    .unwrap_or_default();
                ListItem::new(Line::from(vec![Span::styled(&s.label, style), desc]))
            })
            .collect();

        let block = Block::default()
            .borders(Borders::ALL)
            .title("Suggestions")
            .style(Style::default().fg(theme.text));
        Clear.render(overlay_area, buf);
        List::new(items).block(block).render(overlay_area, buf);
    }

    fn is_capturing(&self) -> bool {
        false
    }

    fn handle_input(&mut self, key: KeyEvent) -> InputResult {
        match key.code {
            KeyCode::Up => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                InputResult::Consumed
            }
            KeyCode::Down => {
                if self.selected + 1 < self.suggestions.len() {
                    self.selected += 1;
                }
                InputResult::Consumed
            }
            KeyCode::Enter => {
                self.confirmed = self.suggestions.get(self.selected).map(|s| s.value.clone());
                InputResult::Consumed
            }
            KeyCode::Esc => {
                self.dismissed = true;
                InputResult::Consumed
            }
            // Any other key dismisses the overlay so it continues to the editor.
            _ => {
                self.dismissed = true;
                InputResult::Consumed
            }
        }
    }

    fn take_result(&mut self) -> OverlayResult {
        if self.dismissed {
            return OverlayResult::Dismissed;
        }
        self.confirmed
            .take()
            .map_or(OverlayResult::Pending, OverlayResult::Confirmed)
    }
}
