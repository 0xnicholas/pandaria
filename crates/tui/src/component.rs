use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// The result of an overlay that has been interacted with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayResult {
    /// Overlay confirmed a selection with this value.
    Confirmed(String),
    /// Overlay was dismissed without selection.
    Dismissed,
    /// Overlay is still active (no result yet).
    Pending,
}

/// Every UI element — widget or overlay — implements this trait.
pub trait Component {
    /// Render into the given area of the buffer.
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// Handle keyboard input when this component has focus.
    /// Default: ignore all input.
    fn handle_input(&mut self, _key: KeyEvent) -> InputResult {
        InputResult::Ignored
    }

    /// Clear any cached rendering state.
    fn invalidate(&mut self) {}

    /// Whether this overlay captures all input (true) or passes through
    /// printable characters to dismiss (false). Default: true (capturing).
    fn is_capturing(&self) -> bool {
        true
    }

    /// If the overlay has completed (confirmed or dismissed), return the result.
    /// Called by the framework after each handle_input to check if the overlay
    /// should be popped. Default: always Pending.
    fn take_result(&mut self) -> OverlayResult {
        OverlayResult::Pending
    }
}

/// Result of a component's input handling.
#[derive(Debug)]
pub enum InputResult {
    /// Input was consumed by this component.
    Consumed,
    /// Input was not handled; pass to next handler.
    Ignored,
}
