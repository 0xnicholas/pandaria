use std::time::Duration;
use thiserror::Error;

/// Classification of stream errors for retry policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamErrorKind {
    /// Transient network/transport layer error (broken pipe, connection reset, unexpected EOF).
    Network,
    /// Provider-reported protocol-level error (Error SSE event, invalid model, etc).
    Protocol,
    /// Data format parsing error (JSON deserialization failure).
    Parse,
}

#[derive(Debug, Clone, Error)]
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
    Serialization(String),

    #[error("unknown provider: {0}")]
    UnknownProvider(String),

    #[error("stream error ({kind:?}): {message}")]
    StreamError { kind: StreamErrorKind, message: String },
}

impl LlmError {
    /// Whether this error is retryable.
    ///
    /// Retryable: RateLimited, Overloaded, Timeout, StreamError::Network.
    /// Non-retryable: everything else.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited(_)
                | Self::Overloaded(_)
                | Self::Timeout(_)
                | Self::StreamError {
                    kind: StreamErrorKind::Network,
                    ..
                }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_retryable_true_variants() {
        assert!(LlmError::RateLimited("429".to_string()).is_retryable());
        assert!(LlmError::Overloaded("busy".to_string()).is_retryable());
        assert!(LlmError::Timeout(Duration::from_secs(30)).is_retryable());
        assert!(LlmError::StreamError {
            kind: StreamErrorKind::Network,
            message: "broken pipe".to_string(),
        }
        .is_retryable());
    }

    #[test]
    fn test_is_retryable_false_variants() {
        assert!(!LlmError::InvalidRequest("bad json".to_string()).is_retryable());
        assert!(!LlmError::ProviderError("internal".to_string()).is_retryable());
        assert!(!LlmError::AuthError("invalid key".to_string()).is_retryable());
        assert!(!LlmError::ContextOverflow("too long".to_string()).is_retryable());
        assert!(!LlmError::Cancelled.is_retryable());
        assert!(!LlmError::StreamError {
            kind: StreamErrorKind::Protocol,
            message: "invalid model".to_string(),
        }
        .is_retryable());
        assert!(!LlmError::StreamError {
            kind: StreamErrorKind::Parse,
            message: "json error".to_string(),
        }
        .is_retryable());
        // Serialization wraps serde_json::Error, so test separately
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        assert!(!LlmError::Serialization(json_err.to_string()).is_retryable());
    }
}
