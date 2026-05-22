use crate::state::ThinkingBlock;
use crate::ui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

impl ThinkingBlock {
    pub fn render(&self, f: &mut Frame, area: Rect, theme: &Theme) {
        let style = Style::default().fg(theme.thinking_text);
        let lines = if self.is_expanded {
            vec![
                Line::from(Span::styled(
                    "💭 Thinking",
                    style.add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    "─".repeat(area.width as usize),
                    Style::default().fg(theme.muted),
                )),
                Line::from(Span::styled(&self.thinking_text, style)),
            ]
        } else {
            vec![Line::from(Span::styled("💭 Thinking...", style))]
        };
        let block = Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(theme.muted));
        f.render_widget(block.clone(), area);
        f.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: true }),
            block.inner(area),
        );
    }
    pub fn toggle(&mut self) {
        self.is_expanded = !self.is_expanded;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_thinking_default_collapsed() {
        let b = ThinkingBlock {
            thinking_text: "test".into(),
            is_expanded: false,
            is_redacted: false,
        };
        assert!(!b.is_expanded);
    }
    #[test]
    fn test_thinking_toggle() {
        let mut b = ThinkingBlock {
            thinking_text: "test".into(),
            is_expanded: false,
            is_redacted: false,
        };
        b.toggle();
        assert!(b.is_expanded);
        b.toggle();
        assert!(!b.is_expanded);
    }
}
