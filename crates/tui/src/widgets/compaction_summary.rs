use crate::state::CompactionSummaryBlock;
use crate::ui::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

pub struct CompactionSummaryWidget<'a> {
    block: &'a CompactionSummaryBlock,
    theme: &'a Theme,
}

impl<'a> CompactionSummaryWidget<'a> {
    pub fn new(block: &'a CompactionSummaryBlock, theme: &'a Theme) -> Self {
        Self { block, theme }
    }
}

impl Widget for CompactionSummaryWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 {
            return;
        }
        let mut lines: Vec<Line> = Vec::new();
        let header = if self.block.expanded {
            "  📦 Compaction Summary ▼"
        } else {
            "  📦 Compaction Summary ▶"
        };
        lines.push(Line::from(Span::styled(
            header,
            Style::default()
                .fg(self.theme.warning)
                .add_modifier(Modifier::BOLD),
        )));
        if self.block.expanded {
            for l in self.block.summary.lines() {
                lines.push(Line::from(Span::styled(
                    format!("    {}", l),
                    Style::default().fg(self.theme.text),
                )));
            }
            if let Some(tokens_before) = self.block.tokens_before
                && let Some(tokens_after) = self.block.tokens_after {
                    lines.push(Line::from(Span::styled(
                        format!("    ({} → {} tokens)", tokens_before, tokens_after),
                        Style::default().fg(self.theme.muted),
                    )));
                }
        }
        Paragraph::new(lines).render(area, buf);
    }
}
