use crate::state::{MessageBlock, MessageRole, MessageStatus, SessionState};
use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

/// Render the chat view with full session data.
/// Called directly from app.rs since session data isn't stored on the component.
pub fn render_chat(f: &mut ratatui::Frame, area: Rect, theme: &Theme, state: &SessionState) {
    let mut all_lines: Vec<Line> = Vec::new();
    for msg in &state.messages {
        match msg.role {
            MessageRole::User => {
                all_lines.push(Line::from(Span::styled("┌ User", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD))));
                for block in &msg.blocks {
                    if let MessageBlock::Text(lines) = block {
                        all_lines.extend(lines.clone());
                    }
                }
                all_lines.push(Line::from(Span::styled("└", Style::default().fg(theme.accent))));
            }
            MessageRole::Assistant => {
                let status_style = match msg.status {
                    MessageStatus::Streaming => Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
                    MessageStatus::Complete => Style::default().fg(theme.success).add_modifier(Modifier::BOLD),
                    MessageStatus::Aborted => Style::default().fg(theme.dim).add_modifier(Modifier::ITALIC),
                    MessageStatus::Error => Style::default().fg(theme.error).add_modifier(Modifier::BOLD),
                };
                let label = match msg.status {
                    MessageStatus::Streaming => "Assistant · streaming",
                    MessageStatus::Complete => "Assistant",
                    MessageStatus::Aborted => "Assistant · interrupted",
                    MessageStatus::Error => "Assistant · error",
                };
                all_lines.push(Line::from(Span::styled(format!("┌ {}", label), status_style)));
                for block in &msg.blocks {
                    match block {
                        MessageBlock::Text(lines) => {
                            if msg.status == MessageStatus::Aborted {
                                all_lines.extend(lines.iter().map(|l| {
                                    Line::from(l.spans.iter().map(|s| Span::styled(s.content.clone(), Style::default().fg(theme.dim))).collect::<Vec<Span>>())
                                }));
                            } else { all_lines.extend(lines.clone()); }
                        }
                        MessageBlock::ToolCall(tc) => {
                            let icon = match tc.state {
                                crate::state::ToolCallState::Pending => "⏳",
                                crate::state::ToolCallState::Success => "✓",
                                crate::state::ToolCallState::Error => "✗",
                            };
                            let line = if tc.is_expanded {
                                format!("  {} Tool: {} ▼", icon, tc.name)
                            } else { format!("  {} Tool: {} ▶", icon, tc.name) };
                            all_lines.push(Line::from(Span::styled(line, Style::default().fg(theme.accent))));
                            if tc.is_expanded {
                                for cl in &tc.content {
                                    all_lines.push(Line::from(Span::styled(format!("    {}", cl.spans.iter().map(|s| s.content.as_ref()).collect::<String>()), Style::default().fg(theme.text))));
                                }
                            }
                        }
                        MessageBlock::Thinking(tb) => {
                            let style = Style::default().fg(theme.thinking_text);
                            if tb.is_expanded {
                                all_lines.push(Line::from(Span::styled("  💭 Thinking:", style.add_modifier(Modifier::BOLD))));
                                for l in tb.thinking_text.lines() {
                                    all_lines.push(Line::from(Span::styled(format!("    {}", l), style)));
                                }
                            } else {
                                all_lines.push(Line::from(Span::styled("  💭 Thinking...", style)));
                            }
                        }
                    }
                }
                if msg.blocks.iter().any(|b| matches!(b, MessageBlock::ToolCall(_))) {
                    all_lines.push(Line::from(Span::styled("─".repeat(area.width.saturating_sub(2) as usize), Style::default().fg(theme.border))));
                }
                all_lines.push(Line::from(Span::styled("└", status_style)));
            }
        }
    }
    if let Some(ref err) = state.error {
        all_lines.push(Line::from(Span::styled(format!("  ⚠ {}: {}", err.code, err.message), Style::default().fg(theme.error).add_modifier(Modifier::BOLD))));
    }
    f.render_widget(Paragraph::new(all_lines).wrap(Wrap { trim: false }), area);
}
