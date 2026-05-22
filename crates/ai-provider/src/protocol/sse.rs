use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;

/// A decoded Server-Sent Event.
#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    /// The event name (e.g. `"message"`).
    /// `None` when no explicit `event:` field was present.
    pub event: Option<String>,
    /// The event payload (concatenated `data:` lines).
    pub data: String,
    /// The ID field if present.
    pub id: Option<String>,
}

impl SseEvent {
    /// Parse the data payload as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, LlmError> {
        serde_json::from_str(&self.data).map_err(|e| LlmError::StreamError {
            kind: crate::StreamErrorKind::Parse,
            message: format!(
                "SSE JSON parse error: {e}; data={}",
                &self.data[..self.data.len().min(256)]
            ),
        })
    }

    /// Convenience: return true if this event carries the provider-specific
    /// stream-termination marker (e.g. OpenAI’s `[DONE]`).
    pub fn is_done_marker(&self) -> bool {
        self.data.trim() == "[DONE]"
    }
}

/// State-machine decoder that turns raw bytes into [`SseEvent`]s.
///
/// Usage:
/// ```ignore
/// let mut decoder = SseDecoder::new(bytes_stream, cancel_token);
/// while let Some(event) = decoder.next().await {
///     let event = event?;
///     // process event.data …
/// }
/// ```
pub struct SseDecoder<S> {
    stream: S,
    buffer: String,
    cancel: CancellationToken,
    done: bool,
}

impl<S> SseDecoder<S> {
    pub fn new(stream: S, cancel: CancellationToken) -> Self {
        Self {
            stream,
            buffer: String::new(),
            cancel,
            done: false,
        }
    }
}

impl<S, E> Stream for SseDecoder<S>
where
    S: Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    type Item = Result<SseEvent, LlmError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // 1. Fast-path: cancellation
        if self.cancel.is_cancelled() {
            self.done = true;
            return Poll::Ready(Some(Err(LlmError::Cancelled)));
        }

        // 2. Try to emit a fully-buffered event before waiting on the network
        while let Some((event, rest)) = try_flush_event(&self.buffer) {
            self.buffer = rest;
            if let Some(event) = event {
                return Poll::Ready(Some(Ok(event)));
            }
            // empty events (just comments / keep-alive) are skipped
        }

        // 3. Need more bytes from the stream
        loop {
            match Pin::new(&mut self.stream).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    // Stream ended – emit any trailing event
                    let trailing = std::mem::take(&mut self.buffer);
                    self.done = true;
                    if let Some(event) = parse_event(&trailing) {
                        return Poll::Ready(Some(Ok(event)));
                    }
                    return Poll::Ready(None);
                }
                Poll::Ready(Some(Ok(bytes))) => {
                    self.buffer.push_str(&String::from_utf8_lossy(&bytes));
                    // After every chunk, attempt to flush events
                    while let Some((event, rest)) = try_flush_event(&self.buffer) {
                        self.buffer = rest;
                        if let Some(event) = event {
                            return Poll::Ready(Some(Ok(event)));
                        }
                    }
                    // If we consumed the whole buffer without yielding an event,
                    // loop back and poll the stream again.
                }
                Poll::Ready(Some(Err(e))) => {
                    self.done = true;
                    return Poll::Ready(Some(Err(LlmError::StreamError {
                        kind: crate::StreamErrorKind::Network,
                        message: format!("SSE network error: {e}"),
                    })));
                }
            }
        }
    }
}

/// Attempt to extract one [`SseEvent`] from the front of `buffer`.
/// Returns `(Some(event), remaining)` or `(None, remaining)` when only
/// comments/keep-alive lines were found, or `None` when no complete event
/// is available yet.
fn try_flush_event(buffer: &str) -> Option<(Option<SseEvent>, String)> {
    // SSE events are separated by an empty line: "\n\n" or "\r\n\r\n".
    // Check "\r\n\r\n" first so it does not get shadowed by "\n\n".
    if let Some(pos) = buffer.find("\r\n\r\n") {
        let raw = &buffer[..pos];
        let rest = buffer[pos + 4..].to_string();
        let event = parse_event(raw);
        return Some((event, rest));
    }
    if let Some(pos) = buffer.find("\n\n") {
        let raw = &buffer[..pos];
        let rest = buffer[pos + 2..].to_string();
        let event = parse_event(raw);
        return Some((event, rest));
    }
    None
}

/// Parse a single SSE event block (without the trailing double-LF).
fn parse_event(raw: &str) -> Option<SseEvent> {
    let mut event: Option<String> = None;
    let mut id: Option<String> = None;
    let mut data_lines: Vec<&str> = Vec::new();

    for line in raw.lines() {
        if line.starts_with(':') {
            // comment / keep-alive – ignore
            continue;
        }
        if let Some((field, value)) = line.split_once(':') {
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => event = Some(value.to_string()),
                "id" => id = Some(value.to_string()),
                "data" => data_lines.push(value),
                _ => {}
            }
        } else if !line.is_empty() {
            // Lines without a colon are treated as field-less data
            data_lines.push(line);
        }
    }

    if data_lines.is_empty() && event.is_none() && id.is_none() {
        return None;
    }

    Some(SseEvent {
        event,
        data: data_lines.join("\n"),
        id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_event() {
        let raw = "data: hello\n";
        let ev = parse_event(raw).unwrap();
        assert_eq!(ev.data, "hello");
        assert_eq!(ev.event, None);
    }

    #[test]
    fn test_parse_with_event_name() {
        let raw = "event: message\ndata: {\"foo\":1}\n";
        let ev = parse_event(raw).unwrap();
        assert_eq!(ev.event, Some("message".to_string()));
        assert_eq!(ev.data, "{\"foo\":1}");
    }

    #[test]
    fn test_parse_multiline_data() {
        let raw = "data: line1\ndata: line2\n";
        let ev = parse_event(raw).unwrap();
        assert_eq!(ev.data, "line1\nline2");
    }

    #[test]
    fn test_done_marker() {
        let ev = SseEvent {
            event: None,
            data: "[DONE]".to_string(),
            id: None,
        };
        assert!(ev.is_done_marker());
    }

    #[test]
    fn test_find_double_lf() {
        assert_eq!("a\n\nb".find("\n\n"), Some(1));
        assert_eq!("a\r\n\r\nb".find("\r\n\r\n"), Some(1));
        assert_eq!("ab".find("\n\n"), None);
    }

    #[test]
    fn test_try_flush_event() {
        let buf = "data: hello\n\nleftover";
        let (ev, rest) = try_flush_event(buf).unwrap();
        let ev = ev.unwrap();
        assert_eq!(ev.data, "hello");
        assert_eq!(rest, "leftover");
    }

    #[test]
    fn test_json_parse() {
        let ev = SseEvent {
            event: None,
            data: r#"{"key":"value"}"#.to_string(),
            id: None,
        };
        let v: serde_json::Value = ev.json().unwrap();
        assert_eq!(v["key"], "value");
    }
}
