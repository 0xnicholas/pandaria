use crate::ui::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// Widget that displays pending queued messages above the editor.
pub struct PendingMessagesWidget<'a> {
    pending: &'a [String],
    theme: &'a Theme,
}

impl<'a> PendingMessagesWidget<'a> {
    pub fn new(pending: &'a [String], theme: &'a Theme) -> Self {
        Self { pending, theme }
    }
}

impl Widget for PendingMessagesWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.pending.is_empty() || area.height < 1 {
            return;
        }

        let count = self.pending.len();
        let label = if count == 1 {
            format!("↑ 1 pending message")
        } else {
            format!("↑ {} pending messages", count)
        };

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            label,
            Style::default()
                .fg(self.theme.accent)
                .add_modifier(Modifier::BOLD),
        )));

        // Show at most 2 pending message previews
        let preview_count = count.min(2);
        for (i, text) in self.pending.iter().take(preview_count).enumerate() {
            let preview: String = text
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(area.width.saturating_sub(6) as usize)
                .collect();
            let ellipsis = if text.lines().count() > 1 || preview.len() < text.len() {
                "..."
            } else {
                ""
            };
            lines.push(Line::from(Span::styled(
                format!("  {}. {}{}", i + 1, preview, ellipsis),
                Style::default().fg(self.theme.dim),
            )));
        }

        if count > preview_count {
            lines.push(Line::from(Span::styled(
                format!("  ... and {} more", count - preview_count),
                Style::default().fg(self.theme.dim),
            )));
        }

        let height = lines.len().min(area.height as usize) as u16;
        let render_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height,
        };
        Paragraph::new(lines).render(render_area, buf);
    }
}
