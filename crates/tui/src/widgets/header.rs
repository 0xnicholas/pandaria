use crate::ui::theme::Theme;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub struct HeaderBar;

impl HeaderBar {
    pub fn render(f: &mut Frame, area: Rect, theme: &Theme, session_name: &str, model: &str) {
        let text = format!("pandaria · session: {} · model: {}", session_name, model);
        let span = Span::styled(text, Style::default().fg(theme.accent).add_modifier(Modifier::BOLD));
        f.render_widget(Paragraph::new(span).alignment(Alignment::Center), area);
    }
}
