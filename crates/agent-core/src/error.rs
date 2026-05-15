use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum AgentError {
    #[error("tool not found: {0}")]
    ToolNotFound(String),

    #[error("tool execution failed: {0}")]
    ToolExecutionFailed(String),

    #[error("hook dispatch error: {0}")]
    HookDispatchError(String),

    #[error("llm error: {0}")]
    LlmError(#[from] ai_provider::LlmError),

    #[error("llm response error: {0}")]
    LlmResponseError(String),

    #[error("context overflow: {0}")]
    ContextOverflow(String),

    #[error("cancelled")]
    Cancelled,

    #[error("compaction failed: {0}")]
    CompactionFailed(String),

    #[error("recovery aborted: {0}")]
    RecoveryAborted(String),

    #[error("persistence error: {0}")]
    Persistence(String),

    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),

    #[error("skill not found: {0}")]
    SkillNotFound(String),

    #[error("skill load failed: {0}")]
    SkillLoadFailed(String),
}

impl AgentError {
    /// Return a sanitized display string with secrets redacted.
    ///
    /// Use this when logging or sending error messages to external systems.
    pub fn to_sanitized_string(&self) -> String {
        observability::sanitize::sanitize_str(&self.to_string())
    }
}

#[derive(Debug, Clone, Error)]
pub enum CompactionError {
    #[error("already compacted")]
    AlreadyCompacted,
    #[error("llm error: {0}")]
    LlmError(String),
}
