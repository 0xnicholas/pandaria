use crate::component::{Component, InputResult, OverlayResult};
use crate::ui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Widget};

pub struct SessionListOverlay {
    sessions: Vec<(String, String)>,
    selected: usize,
    confirmed: Option<String>,
    dismissed: bool,
}

impl SessionListOverlay {
    pub fn new(sessions: Vec<(String, String)>) -> Self {
        Self {
            sessions,
            selected: 0,
            confirmed: None,
            dismissed: false,
        }
    }
}

impl Component for SessionListOverlay {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let theme = Theme::default();
        let items: Vec<ListItem> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(i, (_, title))| {
                if i == self.selected {
                    ListItem::new(Span::styled(
                        format!("▶ {}", title),
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ))
                } else {
                    ListItem::new(Span::styled(
                        format!("  {}", title),
                        Style::default().fg(theme.text),
                    ))
                }
            })
            .collect();
        let block = Block::default()
            .borders(Borders::ALL)
            .title("Sessions")
            .style(Style::default().fg(theme.text));
        let overlay_area = centered_rect(40, 10, area);
        Clear.render(overlay_area, buf);
        List::new(items).block(block).render(overlay_area, buf);
    }

    fn handle_input(&mut self, key: KeyEvent) -> InputResult {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                InputResult::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < self.sessions.len() {
                    self.selected += 1;
                }
                InputResult::Consumed
            }
            KeyCode::Enter => {
                self.confirmed = self.sessions.get(self.selected).map(|(id, _)| id.clone());
                InputResult::Consumed
            }
            KeyCode::Esc => {
                self.dismissed = true;
                InputResult::Consumed
            }
            KeyCode::Delete | KeyCode::Char('d') => {
                self.confirmed = self
                    .sessions
                    .get(self.selected)
                    .map(|(id, _)| format!("delete:{}", id));
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

fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    Rect::new(
        r.x + (r.width.saturating_sub(width)) / 2,
        r.y + (r.height.saturating_sub(height)) / 2,
        width.min(r.width),
        height.min(r.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_list() -> SessionListOverlay {
        SessionListOverlay::new(vec![
            ("s1".into(), "Session One".into()),
            ("s2".into(), "Session Two".into()),
            ("s3".into(), "Session Three".into()),
        ])
    }

    #[test]
    fn test_j_down_navigates() {
        let mut sl = make_list();
        sl.handle_input(KeyEvent::new(
            KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(sl.take_result(), OverlayResult::Pending));
    }

    #[test]
    fn test_enter_confirms_selected() {
        let mut sl = make_list();
        sl.selected = 1;
        sl.handle_input(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(sl.take_result(), OverlayResult::Confirmed("s2".into()));
    }

    #[test]
    fn test_esc_dismisses() {
        let mut sl = make_list();
        sl.handle_input(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(sl.take_result(), OverlayResult::Dismissed));
    }

    #[test]
    fn test_d_sends_delete() {
        let mut sl = make_list();
        sl.selected = 0;
        sl.handle_input(KeyEvent::new(
            KeyCode::Char('d'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(
            sl.take_result(),
            OverlayResult::Confirmed("delete:s1".into())
        );
    }
}
