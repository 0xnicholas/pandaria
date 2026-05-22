use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_INTERVAL_MS: u64 = 80;

pub struct SpinnerWidget {
    pub frame_index: usize,
}

impl SpinnerWidget {
    pub fn new() -> Self {
        Self { frame_index: 0 }
    }
    pub fn tick(&mut self) {
        self.frame_index = (self.frame_index + 1) % SPINNER_FRAMES.len();
    }
    pub fn interval_ms() -> u64 {
        SPINNER_INTERVAL_MS
    }
    pub fn render(&self, f: &mut Frame, area: Rect, visible: bool) {
        if !visible {
            return;
        }
        let frame = SPINNER_FRAMES[self.frame_index];
        f.render_widget(Paragraph::new(Span::styled(frame, Style::default())), area);
    }
}

impl Default for SpinnerWidget {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_spinner_cycles() {
        let mut s = SpinnerWidget::new();
        let init = s.frame_index;
        for _ in 0..SPINNER_FRAMES.len() {
            s.tick();
        }
        assert_eq!(s.frame_index, init);
    }
    #[test]
    fn test_spinner_tick_advances() {
        let mut s = SpinnerWidget::new();
        let before = s.frame_index;
        s.tick();
        assert_ne!(s.frame_index, before);
    }
}
