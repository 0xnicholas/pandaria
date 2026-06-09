use crate::component::{Component, InputResult, OverlayResult};
use crate::ui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Widget};

const COMMANDS: &[(&str, &str)] = &[
    ("/quit", "Quit"),
    ("/new", "New session"),
    ("/switch <id>", "Switch session"),
    ("/list", "List sessions"),
    ("/model <id>", "Select model"),
    ("/clear", "Clear view"),
    ("/connect <url>", "Connect"),
    ("/auth <token>", "Set token"),
    ("/tokens", "Show usage"),
    ("/help", "Help"),
];

pub struct CommandPalette {
    filter: String,
    selected: usize,
    confirmed: Option<String>,
    dismissed: bool,
}

impl Default for CommandPalette {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandPalette {
    pub fn new() -> Self {
        Self {
            filter: String::new(),
            selected: 0,
            confirmed: None,
            dismissed: false,
        }
    }
    fn filtered(&self) -> Vec<(&'static str, &'static str)> {
        if self.filter.is_empty() {
            COMMANDS.to_vec()
        } else {
            COMMANDS
                .iter()
                .filter(|(c, d)| {
                    c.contains(&self.filter)
                        || d.to_lowercase().contains(&self.filter.to_lowercase())
                })
                .copied()
                .collect()
        }
    }
}

impl Component for CommandPalette {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let theme = Theme::default();
        let filtered = self.filtered();
        let items: Vec<ListItem> = filtered
            .iter()
            .enumerate()
            .map(|(i, (cmd, desc))| {
                if i == self.selected {
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            *cmd,
                            Style::default()
                                .fg(theme.accent)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!(" — {}", desc), Style::default().fg(theme.muted)),
                    ]))
                } else {
                    ListItem::new(Line::from(vec![
                        Span::styled(*cmd, Style::default().fg(theme.text)),
                        Span::styled(format!(" — {}", desc), Style::default().fg(theme.muted)),
                    ]))
                }
            })
            .collect();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!("Commands ({})", filtered.len()))
            .style(Style::default().fg(theme.text));
        let h = (filtered.len() as u16 + 2).min(12);
        let overlay_area = Rect::new(
            area.x + (area.width.saturating_sub(50)) / 2,
            area.y + (area.height.saturating_sub(h)) / 3,
            50,
            h,
        );
        Clear.render(overlay_area, buf);
        List::new(items).block(block).render(overlay_area, buf);
    }

    fn handle_input(&mut self, key: KeyEvent) -> InputResult {
        match key.code {
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.selected = 0;
                InputResult::Consumed
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.selected = 0;
                InputResult::Consumed
            }
            KeyCode::Up => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                InputResult::Consumed
            }
            KeyCode::Down => {
                let n = self.filtered().len();
                if n > 0 && self.selected + 1 < n {
                    self.selected += 1;
                }
                InputResult::Consumed
            }
            KeyCode::Enter => {
                let f = self.filtered();
                self.confirmed = f.get(self.selected).map(|(cmd, _)| cmd.to_string());
                InputResult::Consumed
            }
            KeyCode::Esc => {
                self.dismissed = true;
                InputResult::Consumed
            }
            _ => InputResult::Consumed,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_filter_shows_all() {
        let cp = CommandPalette::new();
        assert_eq!(cp.filtered().len(), COMMANDS.len());
    }

    #[test]
    fn test_enter_confirms_selection() {
        let mut cp = CommandPalette::new();
        let result = cp.handle_input(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(result, InputResult::Consumed));
        assert!(matches!(cp.take_result(), OverlayResult::Confirmed(ref v) if v == "/quit"));
    }

    #[test]
    fn test_esc_dismisses() {
        let mut cp = CommandPalette::new();
        cp.handle_input(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(cp.take_result(), OverlayResult::Dismissed));
    }

    #[test]
    fn test_down_arrow_navigates() {
        let mut cp = CommandPalette::new();
        cp.handle_input(KeyEvent::new(
            KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        cp.handle_input(KeyEvent::new(
            KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        let result = cp.handle_input(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(result, InputResult::Consumed));
    }
}
