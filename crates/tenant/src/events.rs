use agent_core::{AgentEvent, AgentEventListener};

/// Bridges `AgentEvent` from `SessionActor` to a `broadcast::Sender`.
pub struct SessionEventBridge {
    tx: tokio::sync::broadcast::Sender<AgentEvent>,
}

impl SessionEventBridge {
    pub fn new(tx: tokio::sync::broadcast::Sender<AgentEvent>) -> Self {
        Self { tx }
    }

    /// Create an `mpsc::Receiver` subscribed to the broadcast stream.
    /// Each call creates an independent subscription.
    pub fn subscribe(&self) -> tokio::sync::mpsc::Receiver<AgentEvent> {
        let (mpsc_tx, mpsc_rx) = tokio::sync::mpsc::channel(128);
        let mut broadcast_rx = self.tx.subscribe();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = mpsc_tx.closed() => break,
                    result = broadcast_rx.recv() => {
                        match result {
                            Ok(event) => {
                                if mpsc_tx.send(event).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => break, // broadcast closed or lagged
                        }
                    }
                }
            }
        });

        mpsc_rx
    }
}

#[async_trait::async_trait]
impl AgentEventListener for SessionEventBridge {
    async fn on_event(&self, event: &AgentEvent) {
        // broadcast::Sender::send is infallible (drops old receivers on lag)
        let _ = self.tx.send(event.clone());
    }
}
