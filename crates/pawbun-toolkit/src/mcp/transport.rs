//! MCP transport layer: stdio and SSE.

use std::io::{BufRead, ErrorKind, Write};

use super::core::protocol::{JsonRpcRequest, JsonRpcResponse};
pub use super::core::transport::{Transport, TransportConfig, TransportError};

// -------------------------------------------------------------------------
// StdioTransport
// -------------------------------------------------------------------------

/// Synchronous stdio transport using a subprocess.
///
/// Each request is serialized to a single JSON line and written to the child stdin.
/// The response is read as a single JSON line from child stdout.
pub struct StdioTransport {
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
    _child: std::process::Child,
}

impl std::fmt::Debug for StdioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioTransport").finish_non_exhaustive()
    }
}

impl StdioTransport {
    /// Spawns a subprocess and creates a stdio transport.
    pub fn new(command: impl AsRef<str>, args: &[impl AsRef<str>]) -> Result<Self, TransportError> {
        let mut child = std::process::Command::new(command.as_ref())
            .args(args.iter().map(|a| a.as_ref()))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .map_err(|e| TransportError::Io {
                message: format!("failed to spawn process: {e}"),
                kind: e.kind(),
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TransportError::Io {
                message: "failed to open child stdin".into(),
                kind: ErrorKind::Other,
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TransportError::Io {
                message: "failed to open child stdout".into(),
                kind: ErrorKind::Other,
            })?;

        Ok(Self {
            stdin,
            stdout: std::io::BufReader::new(stdout),
            _child: child,
        })
    }

    fn write_request(&mut self, req: &JsonRpcRequest) -> Result<(), TransportError> {
        let line =
            serde_json::to_string(req).map_err(|e| TransportError::Serialization(e.to_string()))?;
        writeln!(self.stdin, "{}", line)
            .map_err(|e| TransportError::Io {
                message: format!("failed to write to stdin: {e}"),
                kind: e.kind(),
            })?;
        self.stdin
            .flush()
            .map_err(|e| TransportError::Io {
                message: format!("failed to flush stdin: {e}"),
                kind: e.kind(),
            })?;
        Ok(())
    }

    fn read_response(&mut self) -> Result<JsonRpcResponse, TransportError> {
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .map_err(|e| TransportError::Io {
                message: format!("failed to read from stdout: {e}"),
                kind: e.kind(),
            })?;
        if n == 0 {
            return Err(TransportError::UnexpectedEof);
        }
        serde_json::from_str(&line)
            .map_err(|e| TransportError::Serialization(format!("invalid JSON response: {e}")))
    }
}

impl Transport for StdioTransport {
    fn request(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, TransportError> {
        self.write_request(&req)?;
        self.read_response()
    }

    fn close(self: Box<Self>) -> Result<(), TransportError> {
        drop(self.stdin);
        // Child will be reaped when _child is dropped.
        Ok(())
    }
}

// -------------------------------------------------------------------------
// SseTransport
// -------------------------------------------------------------------------

#[cfg(feature = "http")]
mod sse {
    use super::*;
    use crate::mcp::core::protocol::JsonRpcId;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    /// Maximum size for the SSE parser buffer before truncation.
    const MAX_SSE_BUFFER: usize = 1024 * 1024;

    /// SSE transport for remote MCP servers.
    ///
    /// Implements the full MCP SSE handshake:
    /// 1. Opens an SSE long-polling connection to the given URL.
    /// 2. Reads the `endpoint` event to obtain the POST URL for JSON-RPC requests.
    /// 3. Sends requests via HTTP POST and receives responses via SSE events,
    ///    correlating them by JSON-RPC `id`.
    ///
    /// The transport automatically retries the SSE connection with exponential
    /// backoff on transient failures.
    pub struct SseTransport {
        /// POST endpoint discovered during SSE handshake.
        post_url: tokio::sync::watch::Receiver<Option<String>>,
        /// Pending response channels keyed by JSON-RPC id.
        routes: Arc<tokio::sync::Mutex<HashMap<Option<JsonRpcId>, tokio::sync::oneshot::Sender<JsonRpcResponse>>>>,
        /// Async HTTP client (used inside our Tokio runtime).
        client: reqwest::Client,
        /// Tokio runtime that drives the SSE background task and async I/O.
        runtime: tokio::runtime::Runtime,
        /// Signal to cancel the background SSE reader loop for graceful shutdown.
        cancel_tx: tokio::sync::watch::Sender<bool>,
    }

    impl std::fmt::Debug for SseTransport {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("SseTransport").finish_non_exhaustive()
        }
    }

    impl SseTransport {
        /// Creates a new SSE transport and initiates the SSE handshake in a background Tokio task.
        pub fn new(url: impl Into<String>) -> Result<Self, TransportError> {
            Self::new_with_retry(url, 5, 1_000)
        }

        /// Creates a new SSE transport with configurable retry parameters.
        pub fn new_with_retry(url: impl Into<String>, max_retries: u32, initial_backoff_ms: u64) -> Result<Self, TransportError> {
            let url = url.into();

            #[cfg(not(test))]
            crate::tools::url_utils::validate_url(&url)
                .map_err(|e| TransportError::Http(format!("SSRF protection: {e}")))?;

            let runtime = tokio::runtime::Runtime::new().map_err(|e| TransportError::Io {
                message: format!("failed to create tokio runtime: {e}"),
                kind: ErrorKind::Other,
            })?;

            let client = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(10))
                .no_proxy()
                .build()
                .map_err(|e| TransportError::Io {
                    message: format!("failed to build HTTP client: {e}"),
                    kind: ErrorKind::Other,
                })?;

            let (post_url_tx, post_url_rx) = tokio::sync::watch::channel(None);
            let routes = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
            let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);

            let routes_task = routes.clone();
            let client_task = client.clone();

            runtime.spawn(async move {
                sse_reader_loop(client_task, url, post_url_tx, routes_task, cancel_rx, max_retries, initial_backoff_ms).await;
            });

            Ok(Self {
                post_url: post_url_rx,
                routes,
                client,
                runtime,
                cancel_tx,
            })
        }
    }

    impl Transport for SseTransport {
        fn request(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, TransportError> {
            let is_notification = req.id.is_none();

            self.runtime.block_on(async {
                // Wait for the SSE handshake to provide the POST endpoint.
                let post_url = tokio::time::timeout(Duration::from_secs(30), async {
                    loop {
                        if let Some(url) = self.post_url.borrow().as_ref() {
                            return url.clone();
                        }
                        if self.post_url.changed().await.is_err() {
                            return String::new(); // channel closed
                        }
                    }
                })
                .await
                .map_err(|_| {
                    TransportError::Http("SSE endpoint handshake timed out".into())
                })?;

                if post_url.is_empty() {
                    return Err(TransportError::Http(
                        "SSE endpoint handshake failed (channel closed)".into(),
                    ));
                }

                let body = serde_json::to_string(&req)
                    .map_err(|e| TransportError::Serialization(e.to_string()))?;

                // Notifications have no id and do not expect a response.
                if is_notification {
                    self.client
                        .post(&post_url)
                        .header("Content-Type", "application/json")
                        .body(body)
                        .send()
                        .await
                        .map_err(|e| TransportError::Http(format!("POST failed: {e}")))?;
                    return Ok(JsonRpcResponse {
                        jsonrpc: "2.0".into(),
                        id: None,
                        result: None,
                        error: None,
                    });
                }

                // Setup a channel to receive the response routed by id.
                let id = req.id.clone();
                let (tx, rx) = tokio::sync::oneshot::channel();
                self.routes.lock().await.insert(id.clone(), tx);

                // Send the request via POST.
                let resp = self
                    .client
                    .post(&post_url)
                    .header("Content-Type", "application/json")
                    .body(body)
                    .send()
                    .await
                    .map_err(|e| TransportError::Http(format!("POST failed: {e}")))?;

                if !resp.status().is_success() {
                    self.routes.lock().await.remove(&id);
                    return Err(TransportError::Http(format!(
                        "POST endpoint returned {}",
                        resp.status()
                    )));
                }

                // Wait for the matching response to arrive via SSE (routed by the background task).
                let result = tokio::time::timeout(Duration::from_secs(60), rx).await;
                match result {
                    Ok(Ok(response)) => Ok(response),
                    Ok(Err(_)) => {
                        self.routes.lock().await.remove(&id);
                        Err(TransportError::Http(
                            "SSE response channel closed".into(),
                        ))
                    }
                    Err(_) => {
                        self.routes.lock().await.remove(&id);
                        Err(TransportError::Http(
                            "SSE response timed out after 60s".into(),
                        ))
                    }
                }
            })
        }

        fn close(self: Box<Self>) -> Result<(), TransportError> {
            self.cancel_tx.send_replace(true);
            self.runtime.block_on(async {
                let _ = tokio::time::timeout(Duration::from_secs(5), async {
                    // Give the background task a moment to drain pending routes
                    // and close the SSE connection gracefully.
                })
                .await;
            });
            self.runtime.shutdown_timeout(Duration::from_secs(5));
            Ok(())
        }
    }

    // ------------------------------------------------------------------
    // SSE background reader loop (with retry)
    // ------------------------------------------------------------------

    async fn drain_pending_routes(
        routes: &tokio::sync::Mutex<HashMap<Option<JsonRpcId>, tokio::sync::oneshot::Sender<JsonRpcResponse>>>,
    ) {
        let mut guard = routes.lock().await;
        for (id, tx) in guard.drain() {
            let _ = tx.send(JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: None,
                error: Some(crate::mcp::core::protocol::JsonRpcError {
                    code: -32000,
                    message: "SSE connection lost, request not resent".into(),
                    data: None,
                }),
            });
        }
    }

    async fn sse_reader_loop(
        client: reqwest::Client,
        url: String,
        post_url_tx: tokio::sync::watch::Sender<Option<String>>,
        routes: Arc<tokio::sync::Mutex<HashMap<Option<JsonRpcId>, tokio::sync::oneshot::Sender<JsonRpcResponse>>>>,
        mut cancel_rx: tokio::sync::watch::Receiver<bool>,
        max_retries: u32,
        initial_backoff_ms: u64,
    ) {
        let mut backoff = Duration::from_millis(initial_backoff_ms);
        let max_backoff = Duration::from_secs(60);
        let mut retry_count = 0u32;

        loop {
            tokio::select! {
                _ = cancel_rx.changed() => {
                    drain_pending_routes(&routes).await;
                    post_url_tx.send_replace(None);
                    break;
                }
                result = sse_reader_task(
                    client.clone(),
                    url.clone(),
                    post_url_tx.clone(),
                    routes.clone(),
                ) => {
                    // Connection closed (graceful or error). Drain pending requests.
                    drain_pending_routes(&routes).await;
                    post_url_tx.send_replace(None);

                    match result {
                        Ok(()) => {
                            // Graceful close. Reset backoff and retry immediately
                            // in case the server closed temporarily.
                            backoff = Duration::from_millis(initial_backoff_ms);
                            retry_count = 0;
                        }
                        Err(e) => {
                            retry_count += 1;
                            if max_retries > 0 && retry_count > max_retries {
                                #[cfg(feature = "tracing")]
                            tracing::error!(error = %e, retries = retry_count, "SSE connection error, max retries exceeded");
                            #[cfg(not(feature = "tracing"))]
                            eprintln!("SSE connection error, max retries exceeded: {e}");
                                break;
                            }
                            #[cfg(feature = "tracing")]
                            tracing::warn!(error = %e, backoff = ?backoff, retry = retry_count, "SSE connection error, will retry");
                            #[cfg(not(feature = "tracing"))]
                            eprintln!("SSE connection error, will retry (backoff={backoff:?}, retry={retry_count}): {e}");
                            tokio::time::sleep(backoff).await;
                            backoff = std::cmp::min(backoff * 2, max_backoff);
                        }
                    }
                }
            }
        }
    }

    async fn sse_reader_task(
        client: reqwest::Client,
        url: String,
        post_url_tx: tokio::sync::watch::Sender<Option<String>>,
        routes: Arc<tokio::sync::Mutex<HashMap<Option<JsonRpcId>, tokio::sync::oneshot::Sender<JsonRpcResponse>>>>,
    ) -> Result<(), String> {
        let mut resp = client
            .get(&url)
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .send()
            .await
            .map_err(|e| format!("failed to connect: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let mut parser = SseParser::new();

        loop {
            match resp.chunk().await {
                Ok(Some(bytes)) => {
                    parser.feed(&bytes);
                    while let Some(event) = parser.next_event() {
                        if event.event.as_deref() == Some("endpoint") {
                            let url = event.data();
                            #[cfg(not(test))]
                            crate::tools::url_utils::validate_url(&url)
                                .map_err(|e| format!("SSRF protection: invalid endpoint URL: {e}"))?;
                            post_url_tx.send_replace(Some(url));
                        } else {
                            let data = event.data();
                            if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&data) {
                                let id = resp.id.clone();
                                if let Some(tx) = routes.lock().await.remove(&id) {
                                    let _ = tx.send(resp);
                                }
                            }
                        }
                    }
                }
                Ok(None) => return Ok(()),
                Err(e) => return Err(format!("stream error: {e}")),
            }
        }
    }

    // ------------------------------------------------------------------
    // SSE parser
    // ------------------------------------------------------------------

    struct SseEvent {
        event: Option<String>,
        data: Vec<String>,
        id: Option<String>,
        retry: Option<u64>,
    }

    impl SseEvent {
        fn data(&self) -> String {
            self.data.join("\n")
        }
    }

    struct SseParser {
        buffer: String,
    }

    impl SseParser {
        fn new() -> Self {
            Self {
                buffer: String::new(),
            }
        }

        fn feed(&mut self, bytes: &[u8]) {
            self.buffer.push_str(&String::from_utf8_lossy(bytes));
            // Defensive truncation: prevent a malicious / misbehaving server from
            // exhausting memory by never sending an event terminator.
            if self.buffer.len() > MAX_SSE_BUFFER {
                // Truncate to the boundary after the most recent complete event,
                // to avoid leaving a partial event in the buffer.
                let target_len = MAX_SSE_BUFFER / 2;
                if let Some(pos) = self.buffer[target_len..].find("\n\n") {
                    let split = target_len + pos + 2;
                    self.buffer = self.buffer[split..].to_string();
                } else {
                    // No complete event boundary found; clear to avoid corrupt parses.
                    self.buffer.clear();
                }
            }
        }

        fn next_event(&mut self) -> Option<SseEvent> {
            let bytes = self.buffer.as_bytes();
            let mut i = 0;
            while i + 1 < bytes.len() {
                // Look for \n\n or \r\n\r\n terminators.
                let is_lf = bytes[i] == b'\n' && bytes[i + 1] == b'\n';
                let is_crlf = i + 3 < bytes.len()
                    && bytes[i] == b'\r'
                    && bytes[i + 1] == b'\n'
                    && bytes[i + 2] == b'\r'
                    && bytes[i + 3] == b'\n';

                if is_lf || is_crlf {
                    let block = self.buffer[..i].replace("\r\n", "\n");
                    let delim = if is_crlf { 4 } else { 2 };
                    self.buffer = self.buffer[i + delim..].to_string();
                    return Some(parse_block(&block));
                }
                i += 1;
            }
            None
        }
    }

    fn parse_block(block: &str) -> SseEvent {
        let mut event = SseEvent {
            event: None,
            data: Vec::new(),
            id: None,
            retry: None,
        };

        for line in block.lines() {
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("event:") {
                event.event = Some(rest.trim_start().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                event.data.push(rest.trim_start().to_string());
            } else if let Some(rest) = line.strip_prefix("id:") {
                event.id = Some(rest.trim_start().to_string());
            } else if let Some(rest) = line.strip_prefix("retry:") {
                if let Ok(ms) = rest.trim_start().parse::<u64>() {
                    event.retry = Some(ms);
                }
            }
            // Comment lines (starting with ":") are intentionally ignored.
        }

        event
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_sse_parser_single_event() {
            let mut parser = SseParser::new();
            parser.feed(b"event: endpoint\ndata: http://localhost:3000/msg\n\n");
            let event = parser.next_event().unwrap();
            assert_eq!(event.event, Some("endpoint".to_string()));
            assert_eq!(event.data(), "http://localhost:3000/msg");
        }

        #[test]
        fn test_sse_parser_multi_line_data() {
            let mut parser = SseParser::new();
            parser.feed(b"event: message\ndata: {\"jsonrpc\":\"2.0\"\ndata: ,\"id\":1\n\n");
            let event = parser.next_event().unwrap();
            assert_eq!(event.event, Some("message".to_string()));
            assert_eq!(event.data(), "{\"jsonrpc\":\"2.0\"\n,\"id\":1");
        }

        #[test]
        fn test_sse_parser_crlf() {
            let mut parser = SseParser::new();
            parser.feed(b"event: endpoint\r\ndata: http://x\r\n\r\n");
            let event = parser.next_event().unwrap();
            assert_eq!(event.event, Some("endpoint".to_string()));
            assert_eq!(event.data(), "http://x");
        }

        #[test]
        fn test_sse_parser_partial_then_complete() {
            let mut parser = SseParser::new();
            parser.feed(b"event: end");
            assert!(parser.next_event().is_none());
            parser.feed(b"point\ndata: /msg\n\n");
            let event = parser.next_event().unwrap();
            assert_eq!(event.event, Some("endpoint".to_string()));
            assert_eq!(event.data(), "/msg");
        }

        #[test]
        fn test_sse_parser_multiple_events_in_one_feed() {
            let mut parser = SseParser::new();
            parser.feed(b"event: a\ndata: 1\n\nevent: b\ndata: 2\n\n");
            let e1 = parser.next_event().unwrap();
            assert_eq!(e1.event, Some("a".to_string()));
            let e2 = parser.next_event().unwrap();
            assert_eq!(e2.event, Some("b".to_string()));
            assert!(parser.next_event().is_none());
        }

        #[test]
        fn test_sse_parser_id_and_retry_fields() {
            let mut parser = SseParser::new();
            parser.feed(b"event: message\nid: 42\nretry: 5000\ndata: hello\n\n");
            let event = parser.next_event().unwrap();
            assert_eq!(event.event, Some("message".to_string()));
            assert_eq!(event.id, Some("42".to_string()));
            assert_eq!(event.retry, Some(5000));
            assert_eq!(event.data(), "hello");
        }

        #[test]
        fn test_sse_parser_ignores_comments() {
            let mut parser = SseParser::new();
            parser.feed(b": this is a comment\nevent: msg\ndata: hi\n\n");
            let event = parser.next_event().unwrap();
            assert_eq!(event.event, Some("msg".to_string()));
            assert_eq!(event.data(), "hi");
        }

        #[test]
        fn test_sse_parser_truncation_safe() {
            let mut parser = SseParser::new();
            // Feed a massive incomplete event (no terminator).
            let huge = "event: foo\ndata: ".to_string() + &"x".repeat(2 * MAX_SSE_BUFFER);
            parser.feed(huge.as_bytes());
            // No terminator → no event.
            assert!(parser.next_event().is_none());
            // After defensive truncation, a subsequent valid event must still parse.
            parser.feed(b"event: ok\ndata: done\n\n");
            let event = parser.next_event().unwrap();
            assert_eq!(event.event, Some("ok".to_string()));
            assert_eq!(event.data(), "done");
        }

        #[test]
        fn test_sse_transport_full_handshake() {
            use std::io::{Read, Write};
            use std::net::TcpListener;
            use std::thread;
            use std::time::Duration;

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let post_url = format!("http://127.0.0.1:{}/msg", port);

            let (done_tx, done_rx) = std::sync::mpsc::channel();
            thread::spawn(move || {
                // Connection 1: SSE stream
                let (mut sse_stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 8192];
                let n = sse_stream.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..n]);
                assert!(req.contains("GET /sse"));
                assert!(req.contains("text/event-stream"));

                let headers =
                    "HTTP/1.1 200 OK\r\n\
                     Content-Type: text/event-stream\r\n\
                     Cache-Control: no-cache\r\n\
                     Connection: keep-alive\r\n\
                     \r\n";
                sse_stream.write_all(headers.as_bytes()).unwrap();

                // Send endpoint event
                let endpoint_event = format!("event: endpoint\ndata: {}\n\n", post_url);
                sse_stream.write_all(endpoint_event.as_bytes()).unwrap();

                // Connection 2: POST request
                let (mut post_stream, _) = listener.accept().unwrap();
                // Read the full POST request (headers + body).  The body may arrive
                // in a second packet, so loop until we see the body.
                let mut post_req = String::new();
                loop {
                    let n = post_stream.read(&mut buf).unwrap();
                    post_req.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if post_req.contains("tools/list") && post_req.contains("\r\n\r\n") {
                        break;
                    }
                    if n == 0 {
                        break;
                    }
                }
                assert!(post_req.contains("POST /msg"));
                assert!(post_req.contains("tools/list"));

                // Respond to the POST so reqwest sees success
                post_stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .unwrap();

                // Give reqwest a moment to process the POST response
                thread::sleep(Duration::from_millis(50));

                // Send JSON-RPC response back through the SSE stream
                let sse_resp = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[]}}\n\n";
                sse_stream.write_all(sse_resp.as_bytes()).unwrap();
                done_tx.send(()).unwrap();
            });

            let mut transport = SseTransport::new(format!("http://127.0.0.1:{}/sse", port)).unwrap();
            let req = crate::mcp::core::protocol::JsonRpcRequest::new(
                1i64,
                "tools/list",
                None,
            );
            let resp = transport.request(req).unwrap();
            assert_eq!(resp.id, Some(crate::mcp::core::protocol::JsonRpcId::Number(1)));
            assert!(resp.result.is_some());

            done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }
}

#[cfg(feature = "http")]
pub use sse::SseTransport;

/// SSE transport stub when the `http` feature is disabled.
#[cfg(not(feature = "http"))]
pub struct SseTransport {
    url: String,
}

#[cfg(not(feature = "http"))]
impl std::fmt::Debug for SseTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SseTransport")
            .field("url", &self.url)
            .finish()
    }
}

#[cfg(not(feature = "http"))]
impl SseTransport {
    /// Creates a new SSE transport placeholder.
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }
}

#[cfg(not(feature = "http"))]
impl Transport for SseTransport {
    fn request(&mut self, _req: JsonRpcRequest) -> Result<JsonRpcResponse, TransportError> {
        Err(TransportError::Http(
            "SseTransport requires the `http` feature to be enabled.".into(),
        ))
    }

    fn close(self: Box<Self>) -> Result<(), TransportError> {
        Ok(())
    }
}
