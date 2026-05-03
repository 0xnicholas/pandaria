use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub struct InputBar {
    pub buffer: String,
    pub cursor_pos: usize,
    pub history: Vec<String>,
    pub history_index: Option<usize>,
}

impl InputBar {
    pub fn new() -> Self { Self { buffer: String::new(), cursor_pos: 0, history: Vec::new(), history_index: None } }
    pub fn insert_char(&mut self, ch: char) { self.buffer.insert(self.cursor_pos, ch); self.cursor_pos += 1; }
    pub fn delete_backward(&mut self) { if self.cursor_pos > 0 { self.cursor_pos -= 1; self.buffer.remove(self.cursor_pos); } }
    pub fn move_cursor_left(&mut self) { if self.cursor_pos > 0 { self.cursor_pos -= 1; } }
    pub fn move_cursor_right(&mut self) { if self.cursor_pos < self.buffer.len() { self.cursor_pos += 1; } }
    pub fn move_cursor_home(&mut self) { self.cursor_pos = 0; }
    pub fn move_cursor_end(&mut self) { self.cursor_pos = self.buffer.len(); }
    pub fn clear(&mut self) { self.buffer.clear(); self.cursor_pos = 0; }
    pub fn set_text(&mut self, text: &str) { self.buffer = text.to_string(); self.cursor_pos = self.buffer.len(); }
    pub fn take_text(&mut self) -> String {
        let text = self.buffer.clone(); self.history.push(text.clone()); self.history_index = None; self.clear(); text
    }
    pub fn history_prev(&mut self) {
        if self.history.is_empty() { return; }
        let idx = match self.history_index { None => self.history.len().saturating_sub(1), Some(i) if i > 0 => i - 1, _ => return };
        self.history_index = Some(idx);
        self.buffer = self.history[idx].clone(); self.cursor_pos = self.buffer.len();
    }
    pub fn history_next(&mut self) {
        match self.history_index {
            None => return,
            Some(i) if i + 1 < self.history.len() => { self.history_index = Some(i + 1); self.buffer = self.history[i + 1].clone(); }
            _ => { self.history_index = None; self.buffer.clear(); }
        }
        self.cursor_pos = self.buffer.len();
    }
    pub fn render(&self, f: &mut Frame, area: Rect, theme: &Theme, busy: bool, _focused: bool) {
        let prompt = if busy { Span::styled("  Interrupt (Esc)… ", Style::default().fg(theme.warning)) } else { Span::styled("  > ", Style::default().fg(theme.accent)) };
        let text = if self.buffer.is_empty() && !busy {
            Span::styled("Write a message or /command...", Style::default().fg(theme.dim))
        } else { Span::styled(&self.buffer, Style::default().fg(theme.text)) };
        let block = Block::default().borders(Borders::TOP).border_style(Style::default().fg(theme.border));
        f.render_widget(Paragraph::new(Line::from(vec![prompt, text])).block(block), area);
    }
}

impl Default for InputBar {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_insert() { let mut b = InputBar::new(); b.insert_char('h'); assert_eq!(b.buffer, "h"); }
    #[test] fn test_delete() { let mut b = InputBar::new(); b.set_text("hi"); b.delete_backward(); assert_eq!(b.buffer, "h"); }
    #[test] fn test_clear() { let mut b = InputBar::new(); b.set_text("t"); b.clear(); assert!(b.buffer.is_empty()); }
    #[test] fn test_take_pushes_history() { let mut b = InputBar::new(); b.set_text("m"); let t = b.take_text(); assert_eq!(t, "m"); assert_eq!(b.history.len(), 1); }
    #[test] fn test_history_nav() { let mut b = InputBar::new(); b.set_text("a"); b.take_text(); b.set_text("b"); b.take_text(); b.history_prev(); assert_eq!(b.buffer, "b"); }
}
