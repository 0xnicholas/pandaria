use axum::response::{sse::Event, IntoResponse, Sse};
use futures::stream::Stream;
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::types::ServerEvent;

/// 将 `mpsc::Receiver<ServerEvent>` 转为 axum SSE 响应。
/// 当底层 HTTP 流被丢弃（客户端断开连接）时，关联的 background task 会被自动中止。
pub struct SseStream {
    rx: tokio::sync::mpsc::Receiver<ServerEvent>,
    abort: tokio::task::AbortHandle,
}

impl SseStream {
    pub fn new(rx: tokio::sync::mpsc::Receiver<ServerEvent>, abort: tokio::task::AbortHandle) -> Self {
        Self { rx, abort }
    }
}

impl IntoResponse for SseStream {
    fn into_response(self) -> axum::response::Response {
        Sse::new(EventStream { rx: self.rx, abort: Some(self.abort) })
            .keep_alive(
                axum::response::sse::KeepAlive::new()
                    .interval(std::time::Duration::from_secs(15))
                    .text("ping"),
            )
            .into_response()
    }
}

struct EventStream {
    rx: tokio::sync::mpsc::Receiver<ServerEvent>,
    abort: Option<tokio::task::AbortHandle>,
}

impl Drop for EventStream {
    fn drop(&mut self) {
        if let Some(abort) = self.abort.take() {
            abort.abort();
        }
    }
}

impl Stream for EventStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(event)) => {
                let event_type = event_type_name(&event);
                let data = match serde_json::to_string(&event) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::error!(error = %e, event_type, "failed to serialize SSE event");
                        return Poll::Ready(Some(Ok(Event::default()
                            .event("error")
                            .data(r#"{"type":"error","code":"internal","message":"event serialization failed"}"#))));
                    }
                };
                Poll::Ready(Some(Ok(Event::default()
                    .event(event_type)
                    .data(data))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn event_type_name(event: &ServerEvent) -> &'static str {
    match event {
        ServerEvent::MessageStart { .. } => "message_start",
        ServerEvent::TextDelta { .. } => "text_delta",
        ServerEvent::ThinkingDelta { .. } => "thinking_delta",
        ServerEvent::ToolCallStarted { .. } => "tool_call_started",
        ServerEvent::ToolCallDelta { .. } => "tool_call_delta",
        ServerEvent::ToolCallDone { .. } => "tool_call_done",
        ServerEvent::TurnEnd { .. } => "turn_end",
        ServerEvent::Error { .. } => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sse_stream_send_and_close() {
        let (tx, rx) = tokio::sync::mpsc::channel::<ServerEvent>(4);
        let mut stream = EventStream { rx, abort: None };

        tx.send(ServerEvent::TextDelta { delta: "hello".into() })
            .await
            .unwrap();
        drop(tx);

        let item = futures::StreamExt::next(&mut stream).await;
        assert!(item.is_some());

        let item = futures::StreamExt::next(&mut stream).await;
        assert!(item.is_none());
    }
}
