use std::collections::VecDeque;
use std::time::Instant;

/// Strategy for handling user input while the agent is busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum QueueStrategy {
    /// Interrupt the current turn and send immediately (steer).
    Steer,
    /// Queue the input and auto-send when the current turn ends (followUp).
    #[default]
    FollowUp,
}


/// A single queued input item.
#[derive(Debug, Clone)]
pub struct QueuedInput {
    pub text: String,
    pub is_bash: bool,
    pub timestamp: Instant,
}

/// Input queue for managing messages while the agent is busy.
#[derive(Debug, Clone)]
pub struct InputQueue {
    pending: VecDeque<QueuedInput>,
    strategy: QueueStrategy,
    /// Stores the editor content before opening command palette, for restoration.
    editor_snapshot: Option<String>,
}

impl InputQueue {
    pub fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            strategy: QueueStrategy::default(),
            editor_snapshot: None,
        }
    }

    pub fn strategy(&self) -> QueueStrategy {
        self.strategy
    }

    pub fn set_strategy(&mut self, strategy: QueueStrategy) {
        self.strategy = strategy;
    }

    pub fn toggle_strategy(&mut self) {
        self.strategy = match self.strategy {
            QueueStrategy::Steer => QueueStrategy::FollowUp,
            QueueStrategy::FollowUp => QueueStrategy::Steer,
        };
    }

    /// Queue an input for later sending.
    pub fn enqueue(&mut self, text: String, is_bash: bool) {
        self.pending.push_back(QueuedInput {
            text,
            is_bash,
            timestamp: Instant::now(),
        });
    }

    /// Remove and return the oldest pending input.
    pub fn dequeue(&mut self) -> Option<QueuedInput> {
        self.pending.pop_front()
    }

    /// Peek at the oldest pending input without removing it.
    pub fn peek(&self) -> Option<&QueuedInput> {
        self.pending.front()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Clear all pending inputs.
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// Return a list of pending texts for display.
    pub fn pending_texts(&self) -> Vec<String> {
        self.pending.iter().map(|q| q.text.clone()).collect()
    }

    /// Save current editor content before opening an overlay.
    pub fn snapshot_editor(&mut self, content: String) {
        self.editor_snapshot = Some(content);
    }

    /// Take the saved editor snapshot.
    pub fn take_editor_snapshot(&mut self) -> Option<String> {
        self.editor_snapshot.take()
    }

    /// Restore the last pending item back to the editor (Ctrl+U behavior).
    pub fn restore_to_editor(&mut self) -> Option<String> {
        self.pending.pop_back().map(|q| q.text)
    }
}

impl Default for InputQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enqueue_dequeue() {
        let mut q = InputQueue::new();
        q.enqueue("hello".to_string(), false);
        q.enqueue("world".to_string(), false);
        assert_eq!(q.len(), 2);
        let item = q.dequeue().unwrap();
        assert_eq!(item.text, "hello");
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn test_strategy_toggle() {
        let mut q = InputQueue::new();
        assert_eq!(q.strategy(), QueueStrategy::FollowUp);
        q.toggle_strategy();
        assert_eq!(q.strategy(), QueueStrategy::Steer);
    }

    #[test]
    fn test_restore_to_editor() {
        let mut q = InputQueue::new();
        q.enqueue("first".to_string(), false);
        q.enqueue("second".to_string(), false);
        assert_eq!(q.restore_to_editor(), Some("second".to_string()));
        assert_eq!(q.len(), 1);
    }
}
