use crate::state::ConnectionStatus;
use crate::ui::theme::Theme;
use crate::widgets::spinner::SpinnerWidget;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// Render the status bar with full connection/state data.
/// Called directly from app.rs since connection data changes every frame.
#[allow(clippy::too_many_arguments)]
pub fn render_status_bar(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    connection: &ConnectionStatus,
    busy: bool,
    spinner: &SpinnerWidget,
    input_tokens: u64,
    context_window: Option<u64>,
    model: &str,
) {
    if area.width < 20 {
        return;
    }
    let conn_icon = match connection {
        ConnectionStatus::Connected => Span::styled("●", Style::default().fg(theme.success)),
        ConnectionStatus::Disconnected => Span::styled("○", Style::default().fg(theme.muted)),
        ConnectionStatus::Reconnecting => Span::styled("↻", Style::default().fg(theme.warning)),
    };
    let center = if busy {
        Span::styled(
            crate::widgets::spinner::SPINNER_FRAMES[spinner.frame_index].to_string(),
            Style::default().fg(theme.accent),
        )
    } else {
        Span::styled(model.to_string(), Style::default().fg(theme.muted))
    };
    let gauge = if let Some(window) = context_window {
        let pct = if window > 0 {
            (input_tokens * 100 / window).min(100)
        } else {
            0
        };
        let filled = (pct as usize * area.width.saturating_sub(20) as usize / 100)
            .min(area.width.saturating_sub(20) as usize);
        let bar = format!(
            "[{}{}] {}%",
            "█".repeat(filled),
            "░".repeat(area.width.saturating_sub(20) as usize - filled),
            pct
        );
        Span::styled(bar, Style::default().fg(theme.muted))
    } else {
        Span::styled(model, Style::default().fg(theme.muted))
    };
    let line = Line::from(vec![conn_icon, Span::from(" "), center, Span::from("   "), gauge]);
    Paragraph::new(line)
        .alignment(Alignment::Center)
        .render(area, buf);
}
