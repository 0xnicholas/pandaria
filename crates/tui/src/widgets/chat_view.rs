use crate::state::{MessageBlock, MessageRole, MessageStatus, SessionState};
use crate::ui::theme::Theme;
use crate::widgets::bash_execution::BashExecutionWidget;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// Render the chat view with full session data.
/// Called directly from app.rs since session data isn't stored on the component.
pub fn render_chat(f: &mut ratatui::Frame, area: Rect, theme: &Theme, state: &SessionState) {
    if state.messages.is_empty() && state.error.is_none() {
        render_empty_state(f, area, theme);
        return;
    }
    let mut all_lines: Vec<Line> = Vec::new();
    for msg in &state.messages {
        match msg.role {
            MessageRole::User => {
                all_lines.push(Line::from(Span::styled(
                    "┌ User",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )));
                for block in &msg.blocks {
                    if let MessageBlock::Text(lines) = block {
                        all_lines.extend(lines.clone());
                    }
                }
                all_lines.push(Line::from(Span::styled(
                    "└",
                    Style::default().fg(theme.accent),
                )));
            }
            MessageRole::Assistant => {
                let status_style = match msg.status {
                    MessageStatus::Streaming => Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                    MessageStatus::Complete => Style::default()
                        .fg(theme.success)
                        .add_modifier(Modifier::BOLD),
                    MessageStatus::Aborted => Style::default()
                        .fg(theme.dim)
                        .add_modifier(Modifier::ITALIC),
                    MessageStatus::Error => Style::default()
                        .fg(theme.error)
                        .add_modifier(Modifier::BOLD),
                };
                let label = match msg.status {
                    MessageStatus::Streaming => "Assistant · streaming",
                    MessageStatus::Complete => "Assistant",
                    MessageStatus::Aborted => "Assistant · interrupted",
                    MessageStatus::Error => "Assistant · error",
                };
                all_lines.push(Line::from(Span::styled(
                    format!("┌ {}", label),
                    status_style,
                )));
                for block in &msg.blocks {
                    match block {
                        MessageBlock::Text(lines) => {
                            if msg.status == MessageStatus::Aborted {
                                all_lines.extend(lines.iter().map(|l| {
                                    Line::from(
                                        l.spans
                                            .iter()
                                            .map(|s| {
                                                Span::styled(
                                                    s.content.clone(),
                                                    Style::default().fg(theme.dim),
                                                )
                                            })
                                            .collect::<Vec<Span>>(),
                                    )
                                }));
                            } else {
                                all_lines.extend(lines.clone());
                            }
                        }
                        MessageBlock::ToolCall(tc) => {
                            let icon = match tc.state {
                                crate::state::ToolCallState::Pending => "⏳",
                                crate::state::ToolCallState::Success => "✓",
                                crate::state::ToolCallState::Error => "✗",
                            };
                            let line = if tc.is_expanded {
                                format!("  {} Tool: {} ▼", icon, tc.name)
                            } else {
                                format!("  {} Tool: {} ▶", icon, tc.name)
                            };
                            all_lines.push(Line::from(Span::styled(
                                line,
                                Style::default().fg(theme.accent),
                            )));
                            if tc.is_expanded {
                                for cl in &tc.content {
                                    all_lines.push(Line::from(Span::styled(
                                        format!(
                                            "    {}",
                                            cl.spans
                                                .iter()
                                                .map(|s| s.content.as_ref())
                                                .collect::<String>()
                                        ),
                                        Style::default().fg(theme.text),
                                    )));
                                }
                            }
                        }
                        MessageBlock::Thinking(tb) => {
                            let style = Style::default().fg(theme.thinking_text);
                            if tb.is_expanded {
                                all_lines.push(Line::from(Span::styled(
                                    "  💭 Thinking:",
                                    style.add_modifier(Modifier::BOLD),
                                )));
                                for l in tb.thinking_text.lines() {
                                    all_lines.push(Line::from(Span::styled(
                                        format!("    {}", l),
                                        style,
                                    )));
                                }
                            } else {
                                all_lines.push(Line::from(Span::styled("  💭 Thinking...", style)));
                            }
                        }
                        MessageBlock::BashExecution(be) => {
                            let _widget = BashExecutionWidget::new(be, theme);
                            let icon = match be.exit_code {
                                Some(0) => "✓",
                                Some(_) => "✗",
                                None if be.stderr.is_empty() => "✓",
                                None => "⚠",
                            };
                            let header = if be.expanded {
                                format!("  {} $ {} ▼", icon, be.command)
                            } else {
                                format!("  {} $ {} ▶", icon, be.command)
                            };
                            let color = match be.exit_code {
                                Some(0) => theme.success,
                                Some(_) => theme.error,
                                None if be.stderr.is_empty() => theme.success,
                                None => theme.warning,
                            };
                            all_lines.push(Line::from(Span::styled(
                                header,
                                Style::default().fg(color).add_modifier(Modifier::BOLD),
                            )));
                            if be.expanded {
                                if !be.stdout.is_empty() {
                                    for l in be.stdout.lines() {
                                        all_lines.push(Line::from(Span::styled(
                                            format!("    {}", l),
                                            Style::default().fg(theme.text),
                                        )));
                                    }
                                }
                                if !be.stderr.is_empty() {
                                    all_lines.push(Line::from(Span::styled(
                                        "    ── stderr ──",
                                        Style::default()
                                            .fg(theme.error)
                                            .add_modifier(Modifier::BOLD),
                                    )));
                                    for l in be.stderr.lines() {
                                        all_lines.push(Line::from(Span::styled(
                                            format!("    {}", l),
                                            Style::default().fg(theme.error),
                                        )));
                                    }
                                }
                                if let Some(code) = be.exit_code {
                                    all_lines.push(Line::from(Span::styled(
                                        format!("    [exit code: {}]", code),
                                        Style::default().fg(if code == 0 {
                                            theme.success
                                        } else {
                                            theme.error
                                        }),
                                    )));
                                }
                            }
                        }
                        MessageBlock::CompactionSummary(cs) => {
                            let header = if cs.expanded {
                                "  📦 Compaction Summary ▼"
                            } else {
                                "  📦 Compaction Summary ▶"
                            };
                            all_lines.push(Line::from(Span::styled(
                                header,
                                Style::default()
                                    .fg(theme.warning)
                                    .add_modifier(Modifier::BOLD),
                            )));
                            if cs.expanded {
                                for l in cs.summary.lines() {
                                    all_lines.push(Line::from(Span::styled(
                                        format!("    {}", l),
                                        Style::default().fg(theme.text),
                                    )));
                                }
                                if let (Some(before), Some(after)) =
                                    (cs.tokens_before, cs.tokens_after)
                                {
                                    all_lines.push(Line::from(Span::styled(
                                        format!("    ({} → {} tokens)", before, after),
                                        Style::default().fg(theme.muted),
                                    )));
                                }
                            }
                        }
                    }
                }
                if msg
                    .blocks
                    .iter()
                    .any(|b| matches!(b, MessageBlock::ToolCall(_)))
                {
                    all_lines.push(Line::from(Span::styled(
                        "─".repeat(area.width.saturating_sub(2) as usize),
                        Style::default().fg(theme.border),
                    )));
                }
                all_lines.push(Line::from(Span::styled("└", status_style)));
            }
        }
    }
    if let Some(ref err) = state.error {
        all_lines.push(Line::from(Span::styled(
            format!("  ⚠ {}: {}", err.code, err.message),
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        )));
    }
    f.render_widget(Paragraph::new(all_lines).wrap(Wrap { trim: false }), area);
}

fn render_empty_state(f: &mut ratatui::Frame, area: Rect, theme: &Theme) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Welcome to Pandaria",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Multi-tenant Agent Runtime & Harness",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::ITALIC),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Type a message and press Enter to chat",
            Style::default().fg(theme.muted),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "/new      new session     /model   switch model",
            Style::default().fg(theme.dim),
        )),
        Line::from(Span::styled(
            "/help     show help       Ctrl+P   cycle model",
            Style::default().fg(theme.dim),
        )),
        Line::from(Span::styled(
            "/compact  compact ctx     Ctrl+O   toggle tools",
            Style::default().fg(theme.dim),
        )),
        Line::from(Span::styled(
            "/clear    clear view      Ctrl+T   toggle thinking",
            Style::default().fg(theme.dim),
        )),
    ];

    let height = lines.len() as u16 + 4;
    let width = 52u16;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let block_area = Rect::new(x, y, width.min(area.width), height.min(area.height));

    let inner = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .inner(block_area);

    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border)),
        block_area,
    );

    f.render_widget(
        Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center),
        inner,
    );
}
