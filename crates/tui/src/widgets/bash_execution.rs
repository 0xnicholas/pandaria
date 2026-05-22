use crate::state::BashExecutionBlock;
use crate::ui::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// Widget that renders a bash execution block within a message.
pub struct BashExecutionWidget<'a> {
    block: &'a BashExecutionBlock,
    theme: &'a Theme,
}

impl<'a> BashExecutionWidget<'a> {
    pub fn new(block: &'a BashExecutionBlock, theme: &'a Theme) -> Self {
        Self { block, theme }
    }
}

impl Widget for BashExecutionWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 {
            return;
        }

        let mut lines: Vec<Line> = Vec::new();
        let header = if self.block.expanded {
            format!("  $ {} ▼", self.block.command)
        } else {
            format!("  $ {} ▶", self.block.command)
        };

        let exit_style = match self.block.exit_code {
            Some(0) => Style::default().fg(self.theme.success),
            Some(_) => Style::default().fg(self.theme.error),
            None if self.block.stderr.is_empty() => Style::default().fg(self.theme.success),
            None => Style::default().fg(self.theme.warning),
        };

        lines.push(Line::from(Span::styled(
            header,
            exit_style.add_modifier(Modifier::BOLD),
        )));

        if self.block.expanded {
            if !self.block.stdout.is_empty() {
                for l in self.block.stdout.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("    {}", l),
                        Style::default().fg(self.theme.text),
                    )));
                }
            }
            if !self.block.stderr.is_empty() {
                lines.push(Line::from(Span::styled(
                    "    ── stderr ──",
                    Style::default()
                        .fg(self.theme.error)
                        .add_modifier(Modifier::BOLD),
                )));
                for l in self.block.stderr.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("    {}", l),
                        Style::default().fg(self.theme.error),
                    )));
                }
            }
            if let Some(code) = self.block.exit_code {
                lines.push(Line::from(Span::styled(
                    format!("    [exit code: {}]", code),
                    Style::default().fg(if code == 0 {
                        self.theme.success
                    } else {
                        self.theme.error
                    }),
                )));
            }
        }

        Paragraph::new(lines).render(area, buf);
    }
}
