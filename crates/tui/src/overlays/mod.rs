use ratatui::layout::Rect;
use ratatui::Frame;

pub mod command_palette;
pub mod help;
pub mod model_selector;
pub mod session_list;

pub trait Overlay {
    fn render(&self, f: &mut Frame, area: Rect);
    fn handle_input(&mut self, key: crossterm::event::KeyEvent) -> OverlayAction;
    fn is_capturing(&self) -> bool { true }
}

pub enum OverlayAction { Consumed, Dismiss, Confirm(String), Ignored }

pub struct OverlayStack { overlays: Vec<Box<dyn Overlay>> }

impl OverlayStack {
    pub fn new() -> Self { Self { overlays: Vec::new() } }
    pub fn push(&mut self, overlay: Box<dyn Overlay>) { self.overlays.push(overlay); }
    pub fn pop(&mut self) { self.overlays.pop(); }
    pub fn clear(&mut self) { self.overlays.clear(); }
    pub fn top_mut(&mut self) -> Option<&mut Box<dyn Overlay>> { self.overlays.last_mut() }
    pub fn is_empty(&self) -> bool { self.overlays.is_empty() }
}
