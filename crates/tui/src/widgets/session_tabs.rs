use crate::component::Component;
use crate::state::SessionId;
use crate::ui::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Tabs, Widget};

pub struct SessionTabsWidget {
    theme: Theme,
    sessions: Vec<(SessionId, String)>,
    active: SessionId,
}

impl SessionTabsWidget {
    pub fn new(theme: Theme) -> Self {
        Self { theme, sessions: Vec::new(), active: SessionId::new() }
    }

    pub fn update(&mut self, sessions: Vec<(SessionId, String)>, active: SessionId) {
        self.sessions = sessions;
        self.active = active;
    }
}

impl Component for SessionTabsWidget {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let titles: Vec<Span> = self.sessions.iter().map(|(id, name)| {
            if id == &self.active {
                Span::styled(name.clone(), Style::default().fg(self.theme.accent))
            } else {
                Span::styled(name.clone(), Style::default().fg(self.theme.muted))
            }
        }).chain(std::iter::once(Span::styled("+", Style::default().fg(self.theme.muted)))).collect();
        Tabs::new(titles).style(Style::default().fg(self.theme.text)).render(area, buf);
    }
}
