use crate::overlays::{Overlay, OverlayAction};
use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem};
use ratatui::Frame;

pub struct SessionListOverlay { sessions: Vec<(String, String)>, selected: usize }

impl SessionListOverlay {
    pub fn new(sessions: Vec<(String, String)>) -> Self { Self { sessions, selected: 0 } }
    pub fn selected_id(&self) -> Option<&str> { self.sessions.get(self.selected).map(|(id, _)| id.as_str()) }
}

impl Overlay for SessionListOverlay {
    fn render(&self, f: &mut Frame, _area: Rect) {
        let theme = Theme::default();
        let items: Vec<ListItem> = self.sessions.iter().enumerate().map(|(i, (_, title))| {
            if i == self.selected { ListItem::new(Span::styled(format!("▶ {}", title), Style::default().fg(theme.accent).add_modifier(Modifier::BOLD))) }
            else { ListItem::new(Span::styled(format!("  {}", title), Style::default().fg(theme.text))) }
        }).collect();
        let block = Block::default().borders(Borders::ALL).title("Sessions").style(Style::default().fg(theme.text));
        let area = centered_rect(40, 10, f.area());
        f.render_widget(Clear, area);
        f.render_widget(List::new(items).block(block), area);
    }
    fn handle_input(&mut self, key: crossterm::event::KeyEvent) -> OverlayAction {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => { if self.selected > 0 { self.selected -= 1; } OverlayAction::Consumed }
            KeyCode::Down | KeyCode::Char('j') => { if self.selected + 1 < self.sessions.len() { self.selected += 1; } OverlayAction::Consumed }
            KeyCode::Enter => OverlayAction::Confirm(self.selected_id().unwrap_or("").to_string()),
            KeyCode::Esc => OverlayAction::Dismiss,
            KeyCode::Delete | KeyCode::Char('d') => OverlayAction::Confirm(format!("delete:{}", self.selected_id().unwrap_or(""))),
            _ => OverlayAction::Consumed,
        }
    }
}

fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    Rect::new(r.x + (r.width.saturating_sub(width)) / 2, r.y + (r.height.saturating_sub(height)) / 2, width.min(r.width), height.min(r.height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlays::OverlayAction;

    fn make_list() -> SessionListOverlay {
        SessionListOverlay::new(vec![
            ("s1".into(), "Session One".into()),
            ("s2".into(), "Session Two".into()),
            ("s3".into(), "Session Three".into()),
        ])
    }

    #[test]
    fn test_selected_id_returns_first_by_default() {
        let sl = make_list();
        assert_eq!(sl.selected_id(), Some("s1"));
    }

    #[test]
    fn test_j_down_navigates() {
        let mut sl = make_list();
        let action = sl.handle_input(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('j'), crossterm::event::KeyModifiers::NONE));
        assert!(matches!(action, OverlayAction::Consumed));
        assert_eq!(sl.selected_id(), Some("s2"));
    }

    #[test]
    fn test_k_up_navigates() {
        let mut sl = make_list();
        sl.selected = 2;
        let action = sl.handle_input(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('k'), crossterm::event::KeyModifiers::NONE));
        assert!(matches!(action, OverlayAction::Consumed));
        assert_eq!(sl.selected_id(), Some("s2"));
    }

    #[test]
    fn test_enter_confirms_selected() {
        let mut sl = make_list();
        sl.selected = 1;
        let action = sl.handle_input(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        assert!(matches!(action, OverlayAction::Confirm(ref v) if v == "s2"));
    }

    #[test]
    fn test_d_sends_delete() {
        let mut sl = make_list();
        sl.selected = 0;
        let action = sl.handle_input(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('d'), crossterm::event::KeyModifiers::NONE));
        assert!(matches!(action, OverlayAction::Confirm(ref v) if v == "delete:s1"));
    }

    #[test]
    fn test_esc_dismisses() {
        let mut sl = make_list();
        let action = sl.handle_input(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Esc, crossterm::event::KeyModifiers::NONE));
        assert!(matches!(action, OverlayAction::Dismiss));
    }
}
