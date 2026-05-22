use crate::component::Component;
use crate::ui::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Paragraph, Widget};

pub struct HeaderBar {
    theme: Theme,
    session_name: String,
    model: String,
}

impl HeaderBar {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            session_name: String::new(),
            model: String::new(),
        }
    }

    pub fn update(&mut self, session_name: String, model: String) {
        self.session_name = session_name;
        self.model = model;
    }
}

impl Component for HeaderBar {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let text = format!(
            "pandaria · session: {} · model: {}",
            self.session_name, self.model
        );
        let span = Span::styled(
            text,
            Style::default()
                .fg(self.theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        Paragraph::new(span)
            .alignment(Alignment::Center)
            .render(area, buf);
    }
}
