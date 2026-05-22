use crate::component::Component;
use crate::ui::theme::Theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

#[derive(Clone, PartialEq)]
struct EditorState {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

pub struct Editor {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
    preferred_col: Option<usize>,
    viewport_top: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    kill_ring: Vec<String>,
    last_kill_appended: bool,
    undo_stack: Vec<EditorState>,
    redo_stack: Vec<EditorState>,
    char_jump_target: Option<char>,
    pub theme: Theme,
    pub focused: bool,
    pub busy: bool,
}

impl Editor {
    pub fn new(theme: Theme) -> Self {
        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            preferred_col: None,
            viewport_top: 0,
            history: Vec::new(),
            history_index: None,
            kill_ring: Vec::new(),
            last_kill_appended: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            char_jump_target: None,
            theme,
            focused: true,
            busy: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn current_line_text(&self) -> &str {
        self.lines
            .get(self.cursor_line)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    pub fn text_before_cursor(&self) -> String {
        let mut result = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i < self.cursor_line {
                result.push_str(line);
                result.push('\n');
            } else if i == self.cursor_line {
                let chars: String = line.chars().take(self.cursor_col).collect();
                result.push_str(&chars);
                break;
            }
        }
        result
    }

    pub fn cursor_up(&mut self) {
        self.save_undo_state();
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            let line_len = self.lines[self.cursor_line].chars().count();
            self.cursor_col = self.preferred_col.unwrap_or(self.cursor_col).min(line_len);
        }
    }

    pub fn cursor_down(&mut self) {
        self.save_undo_state();
        if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            let line_len = self.lines[self.cursor_line].chars().count();
            self.cursor_col = self.preferred_col.unwrap_or(self.cursor_col).min(line_len);
        }
    }

    pub fn cursor_left(&mut self) {
        self.save_undo_state();
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].chars().count();
        }
        self.preferred_col = Some(self.cursor_col);
    }

    pub fn cursor_right(&mut self) {
        self.save_undo_state();
        let line_len = self.lines[self.cursor_line].chars().count();
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
        self.preferred_col = Some(self.cursor_col);
    }

    pub fn cursor_word_left(&mut self) {
        self.save_undo_state();
        let text: String = self.current_line_text().to_string();
        let char_indices: Vec<(usize, char)> = text.char_indices().collect();

        if self.cursor_col == 0 && self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].chars().count();
            self.preferred_col = Some(self.cursor_col);
            return;
        }

        let mut pos = self.cursor_col;

        while pos > 0 {
            let ch = char_indices.get(pos - 1).map(|(_, c)| *c).unwrap_or(' ');
            if ch.is_alphanumeric() || ch == '_' {
                break;
            }
            pos -= 1;
        }

        while pos > 0 {
            let ch = char_indices.get(pos - 1).map(|(_, c)| *c).unwrap_or(' ');
            if !ch.is_alphanumeric() && ch != '_' {
                break;
            }
            pos -= 1;
        }

        self.cursor_col = pos;
        self.preferred_col = Some(self.cursor_col);
    }

    pub fn cursor_word_right(&mut self) {
        self.save_undo_state();
        let text: String = self.current_line_text().to_string();
        let char_indices: Vec<(usize, char)> = text.char_indices().collect();
        let len = char_indices.len();

        if self.cursor_col >= len && self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
            self.preferred_col = Some(0);
            return;
        }

        let mut pos = self.cursor_col.min(len);

        let at_word = pos < len && {
            let ch = char_indices[pos].1;
            ch.is_alphanumeric() || ch == '_'
        };

        if at_word {
            while pos < len {
                let ch = char_indices[pos].1;
                if !ch.is_alphanumeric() && ch != '_' {
                    break;
                }
                pos += 1;
            }
        } else {
            while pos < len {
                let ch = char_indices[pos].1;
                if ch.is_alphanumeric() || ch == '_' {
                    break;
                }
                pos += 1;
            }
            while pos < len {
                let ch = char_indices[pos].1;
                if !ch.is_alphanumeric() && ch != '_' {
                    break;
                }
                pos += 1;
            }
        }

        self.cursor_col = pos;
        self.preferred_col = Some(self.cursor_col);
    }

    pub fn cursor_line_start(&mut self) {
        self.save_undo_state();
        self.cursor_col = 0;
        self.preferred_col = Some(0);
    }

    pub fn cursor_line_end(&mut self) {
        self.save_undo_state();
        self.cursor_col = self.lines[self.cursor_line].chars().count();
        self.preferred_col = Some(self.cursor_col);
    }

    pub fn cursor_doc_start(&mut self) {
        self.save_undo_state();
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.preferred_col = Some(0);
    }

    pub fn cursor_doc_end(&mut self) {
        self.save_undo_state();
        self.cursor_line = self.lines.len().saturating_sub(1);
        self.cursor_col = self.lines[self.cursor_line].chars().count();
        self.preferred_col = Some(self.cursor_col);
    }

    pub fn page_up(&mut self) {
        self.save_undo_state();
        self.cursor_line = self.cursor_line.saturating_sub(10);
        let line_len = self.lines[self.cursor_line].chars().count();
        self.cursor_col = self.cursor_col.min(line_len);
    }

    pub fn page_down(&mut self) {
        self.save_undo_state();
        self.cursor_line = (self.cursor_line + 10).min(self.lines.len().saturating_sub(1));
        let line_len = self.lines[self.cursor_line].chars().count();
        self.cursor_col = self.cursor_col.min(line_len);
    }

    pub fn insert_char(&mut self, ch: char) {
        self.save_undo_state();
        let line = &mut self.lines[self.cursor_line];
        let chars: Vec<char> = line.chars().collect();
        let byte_idx: usize = chars
            .iter()
            .take(self.cursor_col)
            .map(|c| c.len_utf8())
            .sum();
        line.insert(byte_idx, ch);
        self.cursor_col += 1;
        self.preferred_col = Some(self.cursor_col);
        self.last_kill_appended = false;
    }

    pub fn insert_newline(&mut self) {
        self.save_undo_state();
        let line = &mut self.lines[self.cursor_line];
        let chars: Vec<char> = line.chars().collect();
        let byte_idx: usize = chars
            .iter()
            .take(self.cursor_col)
            .map(|c| c.len_utf8())
            .sum();
        let remainder = line.split_off(byte_idx);
        self.cursor_line += 1;
        self.lines.insert(self.cursor_line, remainder);
        self.cursor_col = 0;
        self.preferred_col = Some(0);
        self.last_kill_appended = false;
    }

    pub fn insert_text(&mut self, text: &str) {
        self.save_undo_state();
        for ch in text.chars() {
            if ch == '\n' {
                self.insert_newline_internal();
            } else {
                self.insert_char_internal(ch);
            }
        }
    }

    fn insert_char_internal(&mut self, ch: char) {
        let line = &mut self.lines[self.cursor_line];
        let chars: Vec<char> = line.chars().collect();
        let byte_idx: usize = chars
            .iter()
            .take(self.cursor_col)
            .map(|c| c.len_utf8())
            .sum();
        line.insert(byte_idx, ch);
        self.cursor_col += 1;
        self.preferred_col = Some(self.cursor_col);
    }

    fn insert_newline_internal(&mut self) {
        let line = &mut self.lines[self.cursor_line];
        let chars: Vec<char> = line.chars().collect();
        let byte_idx: usize = chars
            .iter()
            .take(self.cursor_col)
            .map(|c| c.len_utf8())
            .sum();
        let remainder = line.split_off(byte_idx);
        self.cursor_line += 1;
        self.lines.insert(self.cursor_line, remainder);
        self.cursor_col = 0;
        self.preferred_col = Some(0);
    }

    pub fn delete_char_backward(&mut self) {
        self.save_undo_state();
        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_line];
            let chars: Vec<char> = line.chars().collect();
            let prev_byte: usize = chars
                .iter()
                .take(self.cursor_col - 1)
                .map(|c| c.len_utf8())
                .sum();
            let curr_byte: usize = chars
                .iter()
                .take(self.cursor_col)
                .map(|c| c.len_utf8())
                .sum();
            line.drain(prev_byte..curr_byte);
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            let line = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].chars().count();
            self.lines[self.cursor_line].push_str(&line);
        }
        self.preferred_col = Some(self.cursor_col);
        self.last_kill_appended = false;
    }

    pub fn delete_char_forward(&mut self) {
        self.save_undo_state();
        let line = &mut self.lines[self.cursor_line];
        let len = line.chars().count();
        if self.cursor_col < len {
            let chars: Vec<char> = line.chars().collect();
            let curr_byte: usize = chars
                .iter()
                .take(self.cursor_col)
                .map(|c| c.len_utf8())
                .sum();
            let next_byte: usize = chars
                .iter()
                .take(self.cursor_col + 1)
                .map(|c| c.len_utf8())
                .sum();
            line.drain(curr_byte..next_byte);
        } else if self.cursor_line + 1 < self.lines.len() {
            let next_line = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next_line);
        }
        self.last_kill_appended = false;
    }

    pub fn delete_word_backward(&mut self) {
        self.save_undo_state();
        let start_col = self.cursor_col;
        let start_line = self.cursor_line;
        self.cursor_word_left();
        let end_col = self.cursor_col;
        let end_line = self.cursor_line;
        self.cursor_col = start_col;
        self.cursor_line = start_line;
        let deleted = self.delete_range(end_line, end_col, start_line, start_col);
        self.push_kill_ring(deleted, false);
    }

    pub fn delete_word_forward(&mut self) {
        self.save_undo_state();
        let start_col = self.cursor_col;
        let start_line = self.cursor_line;
        self.cursor_word_right();
        let end_col = self.cursor_col;
        let end_line = self.cursor_line;
        self.cursor_col = start_col;
        self.cursor_line = start_line;
        let deleted = self.delete_range(start_line, start_col, end_line, end_col);
        self.push_kill_ring(deleted, false);
    }

    pub fn delete_to_line_start(&mut self) {
        self.save_undo_state();
        let deleted = self.delete_range(self.cursor_line, 0, self.cursor_line, self.cursor_col);
        self.cursor_col = 0;
        self.push_kill_ring(deleted, false);
    }

    pub fn delete_to_line_end(&mut self) {
        self.save_undo_state();
        let len = self.lines[self.cursor_line].chars().count();
        let deleted = self.delete_range(self.cursor_line, self.cursor_col, self.cursor_line, len);
        self.push_kill_ring(deleted, true);
    }

    fn delete_range(
        &mut self,
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
    ) -> String {
        if start_line == end_line && start_col <= end_col {
            let line = &mut self.lines[start_line];
            let chars: Vec<char> = line.chars().collect();
            let start_byte: usize = chars.iter().take(start_col).map(|c| c.len_utf8()).sum();
            let end_byte: usize = chars
                .iter()
                .take(end_col.min(chars.len()))
                .map(|c| c.len_utf8())
                .sum();
            let deleted = line[start_byte..end_byte].to_string();
            line.drain(start_byte..end_byte);
            deleted
        } else {
            String::new()
        }
    }

    fn push_kill_ring(&mut self, text: String, append: bool) {
        if text.is_empty() {
            return;
        }
        if append && self.last_kill_appended && !self.kill_ring.is_empty() {
            if let Some(last) = self.kill_ring.last_mut() {
                last.push_str(&text);
            }
        } else {
            self.kill_ring.push(text);
            if self.kill_ring.len() > 10 {
                self.kill_ring.remove(0);
            }
        }
        self.last_kill_appended = append;
    }

    pub fn kill_ring_yank(&mut self) {
        if let Some(text) = self.kill_ring.last().cloned() {
            self.insert_text(&text);
        }
    }

    pub fn kill_ring_yank_pop(&mut self) {
        if self.kill_ring.len() > 1 {
            self.kill_ring.rotate_right(1);
        }
    }

    pub fn undo(&mut self) {
        if let Some(state) = self.undo_stack.pop() {
            let current = EditorState {
                lines: self.lines.clone(),
                cursor_line: self.cursor_line,
                cursor_col: self.cursor_col,
            };
            self.redo_stack.push(current);
            self.lines = state.lines;
            self.cursor_line = state.cursor_line;
            self.cursor_col = state.cursor_col;
        }
    }

    pub fn redo(&mut self) {
        if let Some(state) = self.redo_stack.pop() {
            let current = EditorState {
                lines: self.lines.clone(),
                cursor_line: self.cursor_line,
                cursor_col: self.cursor_col,
            };
            self.undo_stack.push(current);
            self.lines = state.lines;
            self.cursor_line = state.cursor_line;
            self.cursor_col = state.cursor_col;
        }
    }

    fn save_undo_state(&mut self) {
        let state = EditorState {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        };
        if self.undo_stack.last() != Some(&state) {
            self.undo_stack.push(state);
            if self.undo_stack.len() > 100 {
                self.undo_stack.remove(0);
            }
        }
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_index {
            None => self.history.len().saturating_sub(1),
            Some(i) if i > 0 => i - 1,
            _ => return,
        };
        self.history_index = Some(idx);
        self.restore_history_entry(idx);
    }

    pub fn history_next(&mut self) {
        match self.history_index {
            None => return,
            Some(i) if i + 1 < self.history.len() => {
                self.history_index = Some(i + 1);
                self.restore_history_entry(i + 1);
            }
            _ => {
                self.history_index = None;
                self.lines = vec![String::new()];
                self.cursor_line = 0;
                self.cursor_col = 0;
            }
        }
    }

    fn restore_history_entry(&mut self, idx: usize) {
        let text = &self.history[idx];
        self.lines = text.lines().map(|s| s.to_string()).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = self.lines.len().saturating_sub(1);
        self.cursor_col = self.lines[self.cursor_line].chars().count();
    }

    pub fn take_text(&mut self) -> String {
        let text = self.lines.join("\n");
        if !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        self.history_index = None;
        self.lines = vec![String::new()];
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.viewport_top = 0;
        self.undo_stack.clear();
        self.redo_stack.clear();
        text
    }

    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.viewport_top = 0;
        self.history_index = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.char_jump_target = None;
    }

    pub fn set_char_jump_target(&mut self, ch: char) {
        self.char_jump_target = Some(ch);
    }

    pub fn char_jump(&mut self) {
        if let Some(target) = self.char_jump_target {
            self.save_undo_state();
            let line = &self.lines[self.cursor_line];
            let chars: Vec<char> = line.chars().collect();
            if let Some(pos) = chars
                .iter()
                .skip(self.cursor_col + 1)
                .position(|&c| c == target)
            {
                self.cursor_col = self.cursor_col + 1 + pos;
                self.preferred_col = Some(self.cursor_col);
            }
            self.char_jump_target = None;
        }
    }

    pub fn is_waiting_char_jump(&self) -> bool {
        self.char_jump_target.is_some()
    }

    pub fn insert_paste_marker(&mut self, id: usize, line_count: usize) {
        let marker = format!("[paste #{} +{} lines]", id, line_count);
        self.insert_text(&marker);
    }

    pub fn render_buf(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(self.theme.border));

        let inner = block.inner(area);
        block.render(area, buf);

        let max_lines = inner.height as usize;
        let visible_lines: Vec<Line> = if self.lines.iter().all(|l| l.is_empty()) {
            vec![]
        } else {
            self.lines
                .iter()
                .skip(self.viewport_top)
                .take(max_lines.max(1))
                .enumerate()
                .map(|(i, line)| {
                    let is_current_line = self.viewport_top + i == self.cursor_line;
                    if is_current_line && self.focused {
                        let chars: Vec<char> = line.chars().collect();
                        let before: String = chars.iter().take(self.cursor_col).collect();
                        let at_cursor = chars
                            .get(self.cursor_col)
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| " ".to_string());
                        let after: String = chars.iter().skip(self.cursor_col + 1).collect();

                        Line::from(vec![
                            Span::styled(before, Style::default().fg(self.theme.text)),
                            Span::styled(
                                at_cursor,
                                Style::default()
                                    .fg(self.theme.text)
                                    .add_modifier(Modifier::REVERSED),
                            ),
                            Span::styled(after, Style::default().fg(self.theme.text)),
                        ])
                    } else {
                        Line::from(Span::styled(
                            line.clone(),
                            Style::default().fg(self.theme.text),
                        ))
                    }
                })
                .collect()
        };

        if visible_lines.is_empty() {
            let text = if self.busy {
                "Interrupt (Esc)..."
            } else {
                "Write a message · /help for commands · Ctrl+Shift+P palette"
            };
            let placeholder = Span::styled(text, Style::default().fg(self.theme.dim));
            Paragraph::new(Line::from(placeholder))
                .block(Block::default())
                .render(inner, buf);
        } else {
            Paragraph::new(visible_lines)
                .block(Block::default())
                .render(inner, buf);
        }
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new(Theme::default())
    }
}

impl Component for Editor {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_buf(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_char() {
        let mut ed = Editor::new(Theme::default());
        ed.insert_char('h');
        ed.insert_char('i');
        assert_eq!(ed.lines[0], "hi");
        assert_eq!(ed.cursor_col, 2);
    }

    #[test]
    fn test_insert_newline() {
        let mut ed = Editor::new(Theme::default());
        ed.insert_text("hello");
        ed.insert_newline();
        ed.insert_text("world");
        assert_eq!(ed.lines.len(), 2);
        assert_eq!(ed.lines[0], "hello");
        assert_eq!(ed.lines[1], "world");
    }

    #[test]
    fn test_delete_char_backward() {
        let mut ed = Editor::new(Theme::default());
        ed.insert_text("hi");
        ed.delete_char_backward();
        assert_eq!(ed.lines[0], "h");
        assert_eq!(ed.cursor_col, 1);
    }

    #[test]
    fn test_delete_word_backward() {
        let mut ed = Editor::new(Theme::default());
        ed.insert_text("hello world");
        ed.delete_word_backward();
        assert_eq!(ed.lines[0], "hello ");
    }

    #[test]
    fn test_delete_to_line_end() {
        let mut ed = Editor::new(Theme::default());
        ed.insert_text("hello world");
        ed.cursor_line_start();
        ed.delete_to_line_end();
        assert_eq!(ed.lines[0], "");
    }

    #[test]
    fn test_history_navigation() {
        let mut ed = Editor::new(Theme::default());
        ed.insert_text("first");
        ed.take_text();
        ed.insert_text("second");
        ed.take_text();

        ed.history_prev();
        assert_eq!(ed.lines[0], "second");
        ed.history_prev();
        assert_eq!(ed.lines[0], "first");
    }

    #[test]
    fn test_undo() {
        let mut ed = Editor::new(Theme::default());
        ed.insert_text("hello");
        ed.undo();
        assert!(ed.is_empty());
    }

    #[test]
    fn test_cursor_word_movement() {
        let mut ed = Editor::new(Theme::default());
        ed.insert_text("hello world test");
        ed.cursor_word_left();
        assert_eq!(ed.cursor_col, 12);
        ed.cursor_word_left();
        assert_eq!(ed.cursor_col, 6);
        ed.cursor_word_right();
        assert_eq!(ed.cursor_col, 11);
    }
}
