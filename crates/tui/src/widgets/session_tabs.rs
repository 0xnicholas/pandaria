use crate::state::SessionId;
use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Tabs;
use ratatui::Frame;

pub struct SessionTabsWidget;

impl SessionTabsWidget {
    pub fn render(f: &mut Frame, area: Rect, theme: &Theme, sessions: &[(SessionId, String)], active: &SessionId) {
        let titles: Vec<Span> = sessions.iter().map(|(id, name)| {
            if id == active {
                Span::styled(name.clone(), Style::default().fg(theme.accent))
            } else {
                Span::styled(name.clone(), Style::default().fg(theme.muted))
            }
        }).chain(std::iter::once(Span::styled("+", Style::default().fg(theme.muted)))).collect();
        f.render_widget(Tabs::new(titles).style(Style::default().fg(theme.text)), area);
    }
}
