//! SessionEventHub — owns event system + steer/follow-up queues + processor.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::events::{AgentEvent, AgentEventListener};
use crate::types::AgentMessage;

/// Event queue wrapper (pub(crate) — only SessionEventHub and SessionActor's event_sink closure use it).
pub(crate) struct QueuedEvent {
    pub event: AgentEvent,
}

pub struct SessionEventHub {
    listeners: Arc<Mutex<Vec<Arc<dyn AgentEventListener>>>>,
    event_tx: Option<mpsc::Sender<QueuedEvent>>,
    event_processor_handle: Option<JoinHandle<()>>,
    steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
}

impl SessionEventHub {
    pub fn new() -> Self {
        let (tx, mut rx) = mpsc::channel::<QueuedEvent>(1024);
        let listeners: Arc<Mutex<Vec<Arc<dyn AgentEventListener>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let listeners_for_task = listeners.clone();
        let handle = tokio::spawn(async move {
            while let Some(queued) = rx.recv().await {
                let ls: Vec<_> = listeners_for_task
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .cloned()
                    .collect();
                for listener in &ls {
                    let _ = listener.on_event(&queued.event).await;
                }
            }
        });
        Self {
            listeners,
            event_tx: Some(tx),
            event_processor_handle: Some(handle),
            steer_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
        }
    }

    // ── Events ──

    pub fn emit(&self, event: AgentEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(QueuedEvent { event });
        }
    }

    pub fn add_listener(&mut self, listener: Arc<dyn AgentEventListener>) {
        self.listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(listener);
    }

    // ── Steer / Follow-up ──

    pub fn steer(&self, msg: AgentMessage) {
        self.steer_queue
            .lock()
            .expect("steer queue poisoned")
            .push(msg);
    }

    pub fn follow_up(&self, msg: AgentMessage) {
        self.follow_up_queue
            .lock()
            .expect("follow_up queue poisoned")
            .push(msg);
    }

    pub fn drain_steer(&self) -> Vec<AgentMessage> {
        std::mem::take(&mut *self.steer_queue.lock().expect("steer queue poisoned"))
    }

    pub fn drain_follow_up(&self) -> Vec<AgentMessage> {
        std::mem::take(
            &mut *self
                .follow_up_queue
                .lock()
                .expect("follow_up queue poisoned"),
        )
    }

    // ── Internal accessors (pub(crate) — SessionActor uses these to build AgentLoopConfig) ──

    pub(crate) fn event_tx_clone(&self) -> Option<mpsc::Sender<QueuedEvent>> {
        self.event_tx.clone()
    }

    pub(crate) fn steer_queue_clone(&self) -> Arc<Mutex<Vec<AgentMessage>>> {
        self.steer_queue.clone()
    }

    pub(crate) fn follow_up_queue_clone(&self) -> Arc<Mutex<Vec<AgentMessage>>> {
        self.follow_up_queue.clone()
    }

    // ── Lifecycle ──

    pub async fn shutdown(&mut self) {
        // Drop sender so the processor sees channel closed.
        self.event_tx.take();
        if let Some(handle) = self.event_processor_handle.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), handle).await;
        }
    }

    /// Take the event_tx sender without awaiting processor (used in Drop).
    pub(crate) fn take_event_tx(&mut self) -> Option<mpsc::Sender<QueuedEvent>> {
        self.event_tx.take()
    }

    /// Take the processor handle without awaiting (used in Drop).
    /// Drop on JoinHandle lets the task finish naturally when its sender closes.
    pub(crate) fn shutdown_handle(&mut self) -> Option<JoinHandle<()>> {
        self.event_processor_handle.take()
    }
}

impl Default for SessionEventHub {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SessionEventHub {
    fn drop(&mut self) {
        self.event_tx.take();
        let _ = self.event_processor_handle.take();
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::AgentEvent;
    use crate::types::AgentMessage;
    use ai_provider::{Content, UserMessage};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    struct CountingListener(Arc<AtomicUsize>);
    #[async_trait]
    impl AgentEventListener for CountingListener {
        async fn on_event(&self, _: &AgentEvent) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: vec![Content::Text {
                text: text.to_string(),
                text_signature: None,
            }],
            timestamp: SystemTime::now(),
        })
    }

    #[tokio::test]
    async fn event_hub_listener_receives_events() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut hub = SessionEventHub::new();
        hub.add_listener(Arc::new(CountingListener(counter.clone())));
        hub.emit(AgentEvent::StateChanged {
            state: crate::SessionState::Idle,
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn event_hub_steer_drain() {
        let hub = SessionEventHub::new();
        hub.steer(user_msg("s1"));
        hub.steer(user_msg("s2"));
        assert_eq!(hub.drain_steer().len(), 2);
        assert!(hub.drain_steer().is_empty(), "second drain should be empty");
    }

    #[tokio::test]
    async fn event_hub_follow_up_drain() {
        let hub = SessionEventHub::new();
        hub.follow_up(user_msg("f1"));
        assert_eq!(hub.drain_follow_up().len(), 1);
        assert!(hub.drain_follow_up().is_empty());
    }

    #[tokio::test]
    async fn event_hub_shutdown_terminates_processor() {
        let mut hub = SessionEventHub::new();
        hub.shutdown().await;
        hub.emit(AgentEvent::StateChanged {
            state: crate::SessionState::Idle,
        });
    }
}
