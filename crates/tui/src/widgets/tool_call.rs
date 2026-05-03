use crate::state::{ToolCallState, ToolCallWidget};
use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

impl ToolCallWidget {
    pub fn render(&self, f: &mut Frame, area: Rect, theme: &Theme) {
        let border_fg = match self.state {
            ToolCallState::Pending => theme.warning,
            ToolCallState::Success => theme.success,
            ToolCallState::Error => theme.error,
        };
        let title = if self.is_expanded { format!("Tool: {} ▼", self.name) } else { format!("Tool: {} ▶", self.name) };
        let block = Block::default().borders(Borders::ALL)
            .border_style(Style::default().fg(border_fg))
            .title(Span::styled(title, Style::default().fg(border_fg)));
        f.render_widget(block.clone(), area);
        if self.is_expanded {
            let inner = block.inner(area);
            let lines: Vec<ratatui::text::Line> = self.content.clone();
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        }
    }
    pub fn toggle(&mut self) { self.is_expanded = !self.is_expanded; }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;
    #[test]
    fn test_tool_call_default_pending() {
        let w = ToolCallWidget { call_id: "c1".into(), name: "read".into(), state: ToolCallState::Pending, content: vec![], is_expanded: false };
        assert!(matches!(w.state, ToolCallState::Pending));
    }
    #[test]
    fn test_tool_call_toggle() {
        let mut w = ToolCallWidget { call_id: "c1".into(), name: "read".into(), state: ToolCallState::Pending, content: vec![Line::from("r")], is_expanded: false };
        w.toggle(); assert!(w.is_expanded);
    }
}
