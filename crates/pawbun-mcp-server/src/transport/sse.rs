//! SSE (Server-Sent Events) server transport.
//!
//! Implements the MCP SSE transport specification:
//! 1. Client connects to `GET /sse` and receives an `endpoint` event
//!    with the POST URL for JSON-RPC requests.
//! 2. Client sends JSON-RPC requests via `POST /message?sessionId=xxx`.
//! 3. Server routes responses back through that session's SSE stream.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pawbun_toolkit::mcp::{JsonRpcRequest, JsonRpcResponse};
use pawbun_toolkit::mcp::{ServerTransport, TransportError};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot, RwLock};

use axum::{
    extract::{Query, State},
    response::sse::{Event, Sse},
    routing::{get, post},
    Router,
};
use futures::stream::Stream;
use std::convert::Infallible;
use std::pin::Pin;

/// Configuration for the SSE server transport.
#[derive(Debug, Clone)]
pub struct SseServerConfig {
    /// Server bind address (e.g. "127.0.0.1:3000").
    pub bind_addr: String,
    /// Allowed CORS origins.
    pub cors_origins: Vec<String>,
    /// Heartbeat interval in milliseconds.
    pub heartbeat_interval_ms: u64,
    /// Heartbeat text payload.
    pub heartbeat_text: String,
    /// Maximum concurrent SSE connections.
    pub max_connections: usize,
    /// Session time-to-live duration.
    pub session_ttl: Duration,
}

impl Default for SseServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:3000".into(),
            cors_origins: Vec::new(),
            heartbeat_interval_ms: 15_000,
            heartbeat_text: "ping".into(),
            max_connections: 100,
            session_ttl: Duration::from_secs(30),
        }
    }
}

impl SseServerConfig {
    /// Creates a new SSE server config with the given bind address.
    pub fn new(bind_addr: impl Into<String>) -> Self {
        Self {
            bind_addr: bind_addr.into(),
            ..Self::default()
        }
    }

    /// Sets allowed CORS origins.
    pub fn with_cors_origins(mut self, origins: Vec<String>) -> Self {
        self.cors_origins = origins;
        self
    }

    /// Sets heartbeat interval and text.
    pub fn with_heartbeat(mut self, interval_ms: u64, text: impl Into<String>) -> Self {
        self.heartbeat_interval_ms = interval_ms;
        self.heartbeat_text = text.into();
        self
    }

    /// Sets maximum concurrent connections.
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// Sets session time-to-live.
    pub fn with_session_ttl(mut self, ttl: Duration) -> Self {
        self.session_ttl = ttl;
        self
    }
}

/// A request paired with its response channel, tagged by session.
struct TaggedRequest {
    #[allow(dead_code)]
    session_id: String,
    request: JsonRpcRequest,
    response_tx: oneshot::Sender<JsonRpcResponse>,
}

#[derive(Debug)]
struct SseSession {
    last_activity: Instant,
    sender: mpsc::UnboundedSender<JsonRpcResponse>,
}

#[derive(Debug)]
struct AppState {
    /// Channel to send tagged requests from POST handler to recv().
    request_tx: mpsc::UnboundedSender<TaggedRequest>,
    /// Per-session SSE response channels, keyed by session ID.
    sessions: RwLock<HashMap<String, SseSession>>,
    max_connections: usize,
    heartbeat_interval_ms: u64,
    heartbeat_text: String,
}

/// SSE server transport.
pub struct SseServerTransport {
    /// Tagged requests from POST handler.
    request_rx: mpsc::UnboundedReceiver<TaggedRequest>,
    /// Response channel for the currently-being-handled request.
    current_response_tx: Option<oneshot::Sender<JsonRpcResponse>>,
    /// Tokio runtime that owns the axum server.
    runtime: Runtime,
}

impl SseServerTransport {
    /// Creates a new SSE transport with default config.
    pub fn new(bind_addr: &str) -> Result<Self, String> {
        Self::new_with_config(SseServerConfig::new(bind_addr))
    }

    /// Creates a new SSE transport and starts the axum server in a background task.
    pub fn new_with_config(config: SseServerConfig) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("failed to create tokio runtime: {e}"))?;

        let (request_tx, request_rx) = mpsc::unbounded_channel();
        let state = Arc::new(AppState {
            request_tx,
            sessions: RwLock::new(HashMap::new()),
            max_connections: config.max_connections,
            heartbeat_interval_ms: config.heartbeat_interval_ms,
            heartbeat_text: config.heartbeat_text.clone(),
        });

        let app_state = state.clone();
        let addr = config.bind_addr.clone();
        let session_ttl = config.session_ttl;

        // Spawn GC task for expired sessions.
        let gc_state = state.clone();
        runtime.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                let mut sessions = gc_state.sessions.write().await;
                let now = Instant::now();
                sessions.retain(|_, session| now.duration_since(session.last_activity) < session_ttl);
            }
        });

        runtime.spawn(async move {
            let app = build_router(app_state, config.cors_origins);

            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("SSE transport bind failed: {e}");
                    return;
                }
            };

            if let Err(e) = axum::serve(listener, app).await {
                eprintln!("SSE server error: {e}");
            }
        });

        Ok(Self {
            request_rx,
            current_response_tx: None,
            runtime,
        })
    }
}

#[cfg(feature = "http")]
fn build_router(
    state: Arc<AppState>,
    cors_origins: Vec<String>,
) -> Router {
    let mut router = Router::new()
        .route("/sse", get(sse_handler))
        .route("/message", post(message_handler))
        .with_state(state);

    if !cors_origins.is_empty() {
        use tower_http::cors::{Any, CorsLayer};
        let origins: Vec<axum::http::HeaderValue> = cors_origins
            .into_iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        let cors = CorsLayer::new()
            .allow_origin(tower_http::cors::AllowOrigin::list(origins))
            .allow_methods(Any)
            .allow_headers(Any);
        router = router.layer(cors);
    }

    router
}

#[cfg(not(feature = "http"))]
fn build_router(
    _state: Arc<AppState>,
    _cors_origins: Vec<String>,
) -> Router {
    Router::new()
}

impl ServerTransport for SseServerTransport {
    fn recv(&mut self) -> Result<JsonRpcRequest, TransportError> {
        self.runtime.block_on(async {
            match self.request_rx.recv().await {
                Some(tagged) => {
                    self.current_response_tx = Some(tagged.response_tx);
                    Ok(tagged.request)
                }
                None => Err(TransportError::UnexpectedEof),
            }
        })
    }

    fn send(&mut self, resp: JsonRpcResponse) -> Result<(), TransportError> {
        // Notification responses (empty) are suppressed.
        let is_empty_notification =
            resp.id.is_none() && resp.result.is_none() && resp.error.is_none();
        if is_empty_notification {
            return Ok(());
        }

        if let Some(tx) = self.current_response_tx.take() {
            // Route the response to the SSE session via the stored oneshot channel.
            let _ = tx.send(resp);
            Ok(())
        } else {
            Err(TransportError::Io {
                message: "no pending response channel for SSE send".into(),
                kind: ErrorKind::Other,
            })
        }
    }

    fn close(self: Box<Self>) -> Result<(), TransportError> {
        self.runtime.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }
}

// ── Axum handlers ──

#[derive(serde::Deserialize)]
struct MessageQuery {
    #[serde(rename = "sessionId")]
    session_id: String,
}

type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

async fn sse_handler(
    State(state): State<Arc<AppState>>,
) -> Sse<SseStream> {
    let heartbeat_interval_ms = state.heartbeat_interval_ms;
    let heartbeat_text = state.heartbeat_text.clone();

    let max_reached = {
        let sessions = state.sessions.read().await;
        sessions.len() >= state.max_connections
    };

    if max_reached {
        let stream: SseStream = Box::pin(async_stream::stream! {
            yield Ok(Event::default().event("error").data("max connections reached"));
        });
        return Sse::new(stream).keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(Duration::from_millis(heartbeat_interval_ms))
                .text(heartbeat_text),
        );
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::unbounded_channel();

    state.sessions.write().await.insert(
        session_id.clone(),
        SseSession {
            last_activity: Instant::now(),
            sender: tx,
        },
    );

    let state_clone = state.clone();
    let session_id_clone = session_id.clone();

    // Spawn a task that converts channel responses into SSE events
    let stream: SseStream = Box::pin(async_stream::stream! {
        // First event: tell client where to POST
        yield Ok(Event::default()
            .event("endpoint")
            .data(format!("/message?sessionId={}", session_id)));

        // Then stream responses as they arrive
        while let Some(resp) = rx.recv().await {
            let data = serde_json::to_string(&resp).unwrap_or_default();
            yield Ok(Event::default().event("message").data(data));

            // Update last activity
            if let Some(session) = state_clone.sessions.write().await.get_mut(&session_id_clone) {
                session.last_activity = Instant::now();
            }
        }

        // Clean up session when channel closes
        state_clone.sessions.write().await.remove(&session_id_clone);
    });

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_millis(heartbeat_interval_ms))
            .text(heartbeat_text),
    )
}

async fn message_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MessageQuery>,
    body: String,
) -> Result<String, (axum::http::StatusCode, String)> {
    let req: JsonRpcRequest = serde_json::from_str(&body)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e.to_string()))?;

    let is_notification = req.id.is_none();

    if is_notification {
        // Notifications: forward to the request channel for handler processing.
        // No response expected — create a dummy channel that gets dropped.
        let (tx, _rx) = oneshot::channel();
        state
            .request_tx
            .send(TaggedRequest {
                session_id: query.session_id,
                request: req,
                response_tx: tx,
            })
            .map_err(|e| {
                (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            })?;
        return Ok("Accepted".into());
    }

    // For requests with an id: create a oneshot channel, send to request queue,
    // then wait for the handler to produce a response and route it to the
    // session's SSE channel.
    let (response_tx, response_rx) = oneshot::channel();

    // Clone before moving query.session_id into TaggedRequest.
    let session_id_for_spawn = query.session_id.clone();
    // Capture the request id before moving req into TaggedRequest — needed
    // for emitting a JSON-RPC error if the handler drops the response channel.
    let req_id = req.id.clone();

    // Send the tagged request to the transport's recv() queue.
    state
        .request_tx
        .send(TaggedRequest {
            session_id: query.session_id,
            request: req,
            response_tx,
        })
        .map_err(|e| {
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    // Wait for the handler to produce a response (via oneshot from send()),
    // then forward it to the SSE session channel. If the handler drops the
    // response_tx without sending (panic, transport close), emit a
    // JSON-RPC error to the SSE session so the client doesn't hang.
    let state_for_response = state.clone();
    tokio::spawn(respond_or_error_on_close(
        state_for_response,
        session_id_for_spawn,
        req_id,
        response_rx,
    ));

    // Return 202 Accepted — the actual response comes through SSE.
    Ok("Accepted".into())
}

/// Forwards a handler response to the SSE session channel.
///
/// If the response channel closes without a value (handler panic, transport
/// close, shutdown), emits a JSON-RPC `-32603` error response to the SSE
/// session so the client gets a timely failure instead of hanging until the
/// session TTL expires.
///
/// Notifications (`req_id == None`) emit nothing on close, per the MCP spec.
async fn respond_or_error_on_close(
    state: Arc<AppState>,
    session_id: String,
    req_id: Option<pawbun_toolkit::mcp::JsonRpcId>,
    response_rx: oneshot::Receiver<JsonRpcResponse>,
) {
    match response_rx.await {
        Ok(resp) => {
            let mut sessions_guard = state.sessions.write().await;
            if let Some(session) = sessions_guard.get_mut(&session_id) {
                session.last_activity = Instant::now();
                let _ = session.sender.send(resp);
            }
        }
        Err(_) => {
            // Handler dropped response_tx without sending — could be a panic
            // (caught by hook::with_timeout returning default), an explicit
            // early return, or transport.close(). Either way, surface a
            // JSON-RPC error so the client doesn't wait until session TTL.
            if let Some(id) = req_id {
                let error_resp = JsonRpcResponse::error(
                    Some(id),
                    -32603,
                    "Handler closed without response (panic, shutdown, or timeout)",
                );
                let mut sessions_guard = state.sessions.write().await;
                if let Some(session) = sessions_guard.get_mut(&session_id) {
                    session.last_activity = Instant::now();
                    let _ = session.sender.send(error_resp);
                }
            }
        }
    }
}

// ── Tests for respond_or_error_on_close ──

#[cfg(test)]
mod tests {
    use super::*;
    use pawbun_toolkit::mcp::JsonRpcId;
    use tokio::sync::oneshot;

    async fn make_test_state() -> (Arc<AppState>, String, mpsc::UnboundedReceiver<JsonRpcResponse>) {
        let (request_tx, _request_rx) = mpsc::unbounded_channel();
        let (session_tx, session_rx) = mpsc::unbounded_channel();
        let state = Arc::new(AppState {
            request_tx,
            sessions: RwLock::new(HashMap::new()),
            max_connections: 10,
            heartbeat_interval_ms: 1000,
            heartbeat_text: "ping".into(),
        });
        let session_id = "test-session".to_string();
        state.sessions.write().await.insert(
            session_id.clone(),
            SseSession {
                last_activity: Instant::now(),
                sender: session_tx,
            },
        );
        (state, session_id, session_rx)
    }

    #[tokio::test]
    async fn test_respond_or_error_on_close_forwards_response() {
        let (state, session_id, mut session_rx) = make_test_state().await;
        let (tx, rx) = oneshot::channel();

        // Send a response, then close the channel.
        let resp = JsonRpcResponse::ok_result(
            Some(JsonRpcId::Number(42)),
            serde_json::json!({"hello": "world"}),
        );
        tx.send(resp.clone()).unwrap();

        respond_or_error_on_close(state, session_id, Some(JsonRpcId::Number(42)), rx).await;

        let received = session_rx.recv().await.expect("session should receive response");
        assert_eq!(received.id, Some(JsonRpcId::Number(42)));
        assert!(received.error.is_none());
    }

    #[tokio::test]
    async fn test_respond_or_error_on_close_emits_error_on_drop() {
        // Channel closes (response_tx dropped) without sending — simulates
        // handler panic or transport close.
        let (state, session_id, mut session_rx) = make_test_state().await;
        let (tx, rx) = oneshot::channel::<JsonRpcResponse>();
        drop(tx); // simulate handler panic/drop

        respond_or_error_on_close(
            state,
            session_id,
            Some(JsonRpcId::Number(7)),
            rx,
        )
        .await;

        let received = session_rx
            .recv()
            .await
            .expect("session should receive error response");
        assert_eq!(received.id, Some(JsonRpcId::Number(7)));
        let err = received.error.expect("error should be present");
        assert_eq!(err.code, -32603);
        assert!(
            err.message.to_lowercase().contains("closed")
                || err.message.to_lowercase().contains("panic")
                || err.message.to_lowercase().contains("shutdown"),
            "error message should explain the cause: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn test_respond_or_error_on_close_silent_on_notification() {
        // Notifications have no id, so no error response is emitted even
        // if the channel closes — the MCP spec says notifications get no reply.
        let (state, session_id, mut session_rx) = make_test_state().await;
        let (tx, rx) = oneshot::channel::<JsonRpcResponse>();
        drop(tx);

        respond_or_error_on_close(state, session_id, None, rx).await;

        // Channel should be empty (no message emitted for notification).
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(50), session_rx.recv()).await;
        assert!(
            result.is_err() || result.unwrap().is_none(),
            "notification path should not emit any response"
        );
    }

    #[tokio::test]
    async fn test_respond_or_error_on_close_handles_missing_session() {
        // Session has been removed (client disconnected) — should not panic.
        let (state, _session_id, mut session_rx) = make_test_state().await;
        let (tx, rx) = oneshot::channel::<JsonRpcResponse>();
        drop(tx);

        // Use a session_id that doesn't exist in the map.
        respond_or_error_on_close(
            state,
            "nonexistent-session".into(),
            Some(JsonRpcId::Number(1)),
            rx,
        )
        .await;

        // The test's session_rx was registered under "test-session" — no
        // message should be routed there either.
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(50), session_rx.recv()).await;
        assert!(
            result.is_err() || result.unwrap().is_none(),
            "no message should be routed to a session that no longer exists"
        );
    }
}
