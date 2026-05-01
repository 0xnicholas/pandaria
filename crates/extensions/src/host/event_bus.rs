use tokio::sync::broadcast;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(100);

pub struct EventBus<T: Clone + Send + 'static> {
    tx: broadcast::Sender<T>,
}

impl<T: Clone + Send + 'static> EventBus<T> {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Subscribe to all events on this bus
    pub fn subscribe(&self) -> broadcast::Receiver<T> {
        self.tx.subscribe()
    }

    /// Emit an event to all subscribers. Fire-and-forget.
    pub fn emit(&self, event: T) {
        let _ = self.tx.send(event);
    }
}

/// Spawn a handler for each event on the receiver.
/// Returns immediately — handlers run concurrently.
/// Each handler has a 100ms timeout. Panics are caught by JoinHandle.
pub fn spawn_listener<T, F, Fut>(
    mut rx: broadcast::Receiver<T>,
    handler: F,
) -> tokio::task::JoinHandle<()>
where
    T: Clone + Send + 'static,
    F: Fn(T) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let fut = handler(event);
                    let _ = tokio::time::timeout(DEFAULT_TIMEOUT, fut).await;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("EventBus listener lagged by {} messages", n);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_emit_and_receive() {
        let bus = EventBus::new(16);
        let rx = bus.subscribe();
        bus.emit(42);

        let result = tokio::time::timeout(Duration::from_secs(1), async {
            let mut rx = rx;
            rx.recv().await.unwrap()
        })
        .await
        .unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_spawn_listener() {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let bus: EventBus<String> = EventBus::new(16);
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        let _handle = spawn_listener(bus.subscribe(), move |msg: String| {
            let received = received_clone.clone();
            async move {
                received.lock().await.push(msg);
            }
        });

        bus.emit("hello".to_string());
        bus.emit("world".to_string());

        // Give handlers time to process
        tokio::time::sleep(Duration::from_millis(50)).await;

        let msgs = received.lock().await;
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0], "hello");
        assert_eq!(msgs[1], "world");
    }
}
