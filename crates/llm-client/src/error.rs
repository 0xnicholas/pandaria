use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("rate limited: {0}")]
    RateLimited(String),

    #[error("provider overloaded: {0}")]
    Overloaded(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("provider error: {0}")]
    ProviderError(String),

    #[error("request timed out after {0:?}")]
    Timeout(Duration),

    #[error("authentication failed: {0}")]
    AuthError(String),

    #[error("context overflow: {0}")]
    ContextOverflow(String),

    #[error("cancelled")]
    Cancelled,

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("stream error: {0}")]
    StreamError(String),
}

impl LlmError {
    /// Whether this error is retryable (RateLimited, Overloaded, or Timeout).
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited(_) | Self::Overloaded(_) | Self::Timeout(_)
        )
    }
}
