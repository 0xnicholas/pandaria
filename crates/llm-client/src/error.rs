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

    #[error("request timed out")]
    Timeout,

    #[error("cancelled")]
    Cancelled,

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
