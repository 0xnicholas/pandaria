use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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

/// Delivers session events to an external webhook endpoint.
pub struct WebhookEventListener {
    config: crate::manager::WebhookConfig,
    tenant_id: String,
    session_id: String,
    client: reqwest::Client,
    delivery_queue: tokio::sync::mpsc::Sender<DeliveryJob>,
    disabled: AtomicBool,
}

struct DeliveryJob {
    event: AgentEvent,
    delivery_id: uuid::Uuid,
}

impl WebhookEventListener {
    pub fn new(
        config: crate::manager::WebhookConfig,
        tenant_id: String,
        session_id: String,
        client: reqwest::Client,
    ) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<DeliveryJob>(256);
        let url = Arc::new(config.url.clone());
        let secret = Arc::new(config.secret.clone());
        let events_filter = Arc::new(if config.events.is_empty() {
            vec!["turn_end".to_string(), "error".to_string()]
        } else {
            config.events.clone()
        });
        let client_inner = Arc::new(client.clone());
        let disabled = Arc::new(AtomicBool::new(false));
        let disabled_clone = disabled.clone();
        let tenant_id_arc = Arc::new(tenant_id.clone());
        let session_id_arc = Arc::new(session_id.clone());
        let consecutive_failures = Arc::new(tokio::sync::Mutex::new(0usize));

        tokio::spawn(async move {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(5));
            tracing::info!("webhook delivery worker started");

            while let Some(job) = rx.recv().await {
                tracing::info!(delivery_id = %job.delivery_id, "webhook job received");
                if disabled_clone.load(Ordering::SeqCst) {
                    tracing::info!("webhook disabled, skipping job");
                    continue;
                }

                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => break,
                };

                let url = url.clone();
                let secret = secret.clone();
                let events_filter = events_filter.clone();
                let client = client_inner.clone();
                let disabled = disabled_clone.clone();
                let tenant_id = tenant_id_arc.clone();
                let session_id = session_id_arc.clone();
                let consecutive_failures = consecutive_failures.clone();

                tokio::spawn(async move {
                    let _permit = permit;
                    let event_type = event_type_name(&job.event);
                    tracing::info!(event_type = %event_type, "webhook processing job");
                    if !events_filter.contains(&event_type) {
                        tracing::info!(event_type = %event_type, "webhook event filtered out");
                        return;
                    }

                    let body = match build_payload(&job.event, job.delivery_id) {
                        Some(b) => b,
                        None => {
                            tracing::warn!("webhook build_payload returned None");
                            return;
                        }
                    };

                    let mut retries = 0;
                    let max_retries = 3;
                    let mut success = false;

                    while retries <= max_retries {
                        let mut req = client
                            .post(&*url)
                            .header("Content-Type", "application/json")
                            .header("X-Pandaria-Event", &event_type)
                            .header("X-Pandaria-Delivery", job.delivery_id.to_string())
                            .header("X-Pandaria-Session-Id", &*session_id)
                            .header("X-Pandaria-Tenant-Id", &*tenant_id)
                            .body(body.clone());

                        if let Some(ref s) = *secret {
                            let signature = hmac_sha256(s, &body);
                            req = req.header("X-Pandaria-Signature", format!("sha256={}", signature));
                        }

                        tracing::info!(url = %url, "webhook sending request");
                        match req.send().await {
                            Ok(resp) if resp.status().is_success() => {
                                tracing::info!(status = %resp.status(), "webhook delivery success");
                                success = true;
                                break;
                            }
                            Ok(resp) => {
                                tracing::warn!(status = %resp.status(), "webhook delivery non-success");
                                retries += 1;
                                if retries <= max_retries {
                                    let delay = std::time::Duration::from_secs(1 << (retries - 1));
                                    tokio::time::sleep(delay).await;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "webhook delivery request failed");
                                retries += 1;
                                if retries <= max_retries {
                                    let delay = std::time::Duration::from_secs(1 << (retries - 1));
                                    tokio::time::sleep(delay).await;
                                }
                            }
                        }
                    }

                    let mut failures = consecutive_failures.lock().await;
                    if success {
                        *failures = 0;
                    } else {
                        *failures += 1;
                        let count = *failures;
                        drop(failures);
                        tracing::warn!(
                            delivery_id = %job.delivery_id,
                            event_type = %event_type,
                            failures = %count,
                            "webhook delivery failed"
                        );
                        if count >= 10 {
                            tracing::error!(url = %url, "webhook disabled after 10 consecutive failures");
                            disabled.store(true, Ordering::SeqCst);
                        }
                    }
                });
            }
            tracing::info!("webhook delivery worker exited");
        });

        Self {
            config,
            tenant_id,
            session_id,
            client,
            delivery_queue: tx,
            disabled: AtomicBool::new(false),
        }
    }
}

impl Drop for WebhookEventListener {
    fn drop(&mut self) {
        tracing::info!("WebhookEventListener dropped");
    }
}

#[async_trait::async_trait]
impl AgentEventListener for WebhookEventListener {
    async fn on_event(&self, event: &AgentEvent) {
        if self.disabled.load(Ordering::SeqCst) {
            tracing::info!("webhook listener disabled, skipping event");
            return;
        }

        let job = DeliveryJob {
            event: event.clone(),
            delivery_id: uuid::Uuid::new_v4(),
        };

        match self.delivery_queue.try_send(job) {
            Ok(()) => tracing::info!("webhook job queued for delivery"),
            Err(e) => tracing::warn!("webhook job queue failed: {:?}", e),
        }
    }
}

fn event_type_name(event: &AgentEvent) -> String {
    match event {
        AgentEvent::AgentStart => "agent_start".into(),
        AgentEvent::AgentEnd { .. } => "agent_end".into(),
        AgentEvent::TurnStart { .. } => "turn_start".into(),
        AgentEvent::TurnEnd { .. } => "turn_end".into(),
        AgentEvent::MessageStart { .. } => "message_start".into(),
        AgentEvent::MessageUpdate { .. } => "message_update".into(),
        AgentEvent::MessageEnd { .. } => "message_end".into(),
        AgentEvent::ToolExecutionStart { .. } => "tool_execution_start".into(),
        AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update".into(),
        AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end".into(),
        AgentEvent::CompactionStart { .. } => "compaction_start".into(),
        AgentEvent::CompactionEnd { .. } => "compaction_end".into(),
        AgentEvent::AutoRetryStart { .. } => "auto_retry_start".into(),
        AgentEvent::AutoRetryEnd { .. } => "auto_retry_end".into(),
        AgentEvent::Error { .. } => "error".into(),
        AgentEvent::StateChanged { .. } => "state_changed".into(),
        _ => "unknown".into(),
    }
}

fn build_payload(event: &AgentEvent, delivery_id: uuid::Uuid) -> Option<String> {
    use serde_json::json;

    let payload = match event {
        AgentEvent::TurnEnd { turn_index, messages } => {
            let last_assistant = messages.iter().rev().find_map(|m| match m {
                agent_core::AgentMessage::Assistant(a) => Some(a),
                _ => None,
            });
            json!({
                "type": "turn_end",
                "turn_index": turn_index,
                "stop_reason": last_assistant.map(|a| format!("{:?}", a.stop_reason).to_lowercase()),
                "usage": last_assistant.map(|a| json!({
                    "input_tokens": a.usage.input_tokens,
                    "output_tokens": a.usage.output_tokens,
                })),
                "delivery_id": delivery_id,
            })
        }
        AgentEvent::Error { error } => json!({
            "type": "error",
            "code": error.code(),
            "message": error.to_sanitized_string(),
            "delivery_id": delivery_id,
        }),
        AgentEvent::StateChanged { state } => json!({
            "type": "state_changed",
            "state": format!("{:?}", state).to_lowercase(),
            "delivery_id": delivery_id,
        }),
        _ => return None,
    };

    serde_json::to_string(&payload).ok()
}

fn hmac_sha256(secret: &str, body: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(body.as_bytes());
    let result = mac.finalize();
    let bytes = result.into_bytes();
    hex::encode(bytes)
}
