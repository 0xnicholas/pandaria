use crate::client::model::ServerEvent;
use futures_util::StreamExt;
use reqwest::Client;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing;

pub async fn connect(
    client: &Client,
    base_url: &str,
    session_id: &str,
    token: &str,
    tx: mpsc::Sender<ServerEvent>,
) {
    let url = format!("{}/api/v1/sessions/{}/events", base_url, session_id);
    let mut retry_attempt: u32 = 0;

    loop {
        match connect_once(client, &url, token, tx.clone()).await {
            Ok(()) => break,
            Err(e) => {
                retry_attempt += 1;
                let delay = Duration::from_secs(2u64.pow(retry_attempt).min(30));
                tracing::warn!(
                    attempt = retry_attempt,
                    delay_ms = delay.as_millis(),
                    error = %e,
                    "SSE connection lost, reconnecting..."
                );
                let _ = tx.send(ServerEvent::Error {
                    code: "sse_reconnecting".to_string(),
                    message: format!("Reconnecting in {}s...", delay.as_secs()),
                }).await;
                tokio::time::sleep(delay).await;
            }
        }
    }
}

async fn connect_once(
    client: &Client,
    url: &str,
    token: &str,
    tx: mpsc::Sender<ServerEvent>,
) -> Result<(), String> {
    let resp = client.get(url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "text/event-stream")
        .send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("SSE connection failed: HTTP {}", resp.status().as_u16()));
    }
    let byte_stream = resp.bytes_stream();
    let mut event_stream = eventsource_stream::EventStream::new(byte_stream);
    while let Some(result) = event_stream.next().await {
        match result {
            Ok(event) => {
                if let Ok(server_event) = serde_json::from_str::<ServerEvent>(&event.data)
                    && tx.send(server_event).await.is_err()
                {
                    break;
                }
            }
            Err(_) => {
                let _ = tx.send(ServerEvent::Error {
                    code: "sse_parse_error".to_string(),
                    message: "failed to parse SSE event".to_string(),
                }).await;
            }
        }
    }
    Ok(())
}
