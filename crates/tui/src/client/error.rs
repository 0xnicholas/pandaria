use thiserror::Error;

#[derive(Error, Debug)]
pub enum TuiError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },

    #[error("SSE connection error: {0}")]
    SseConnection(String),
}
