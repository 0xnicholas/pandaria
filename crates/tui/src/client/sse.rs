use crate::client::model::ServerEvent;
use futures_util::StreamExt;
use reqwest::Client;
use tokio::sync::mpsc;

pub async fn connect(
    client: &Client,
    base_url: &str,
    session_id: &str,
    token: &str,
    last_event_id: Option<&str>,
    tx: mpsc::Sender<ServerEvent>,
) -> Result<(), String> {
    let url = format!("{}/api/v1/sessions/{}/events", base_url, session_id);
    let mut req = client.get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "text/event-stream");
    if let Some(id) = last_event_id { req = req.header("Last-Event-ID", id); }
    let resp = req.send().await.map_err(|e| e.to_string())?;
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
