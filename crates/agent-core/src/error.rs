use thiserror::Error;

/// Errors that can occur during agent execution.
///
/// **Stability note:** This enum is `#[non_exhaustive]`. Consumers must always
/// include a wildcard arm (`_ => {}`) when matching on `AgentError` to remain
/// forward-compatible as new variants are added.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
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

    #[error("session in error state: {reason}")]
    SessionInError { reason: String },

    #[error("loop strategy is disabled (PANDARIA_DISABLE_CRON=1)")]
    LoopDisabled,

    #[error("goal not met after {attempts} attempts: {criteria:?}")]
    GoalNotMet {
        criteria: Vec<String>,
        attempts: u32,
    },
}

impl AgentError {
    /// Return a stable machine-readable error code for this variant.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ToolNotFound(_) => "tool_not_found",
            Self::ToolExecutionFailed(_) => "tool_execution_failed",
            Self::HookDispatchError(_) => "hook_dispatch_error",
            Self::LlmError(_) => "llm_error",
            Self::LlmResponseError(_) => "llm_response_error",
            Self::ContextOverflow(_) => "context_overflow",
            Self::Cancelled => "cancelled",
            Self::CompactionFailed(_) => "compaction_failed",
            Self::RecoveryAborted(_) => "recovery_aborted",
            Self::Persistence(_) => "persistence_error",
            Self::QuotaExceeded(_) => "quota_exceeded",
            Self::SkillNotFound(_) => "skill_not_found",
            Self::SkillLoadFailed(_) => "skill_load_failed",
            Self::SessionInError { .. } => "session_in_error",
            Self::LoopDisabled => "loop_disabled",
            Self::GoalNotMet { .. } => "goal_not_met",
        }
    }

    /// Return a sanitized display string with secrets redacted.
    ///
    /// Use this when logging or sending error messages to external systems.
    pub fn to_sanitized_string(&self) -> String {
        crate::utils::sanitize::sanitize_str(&self.to_string())
    }
}

#[derive(Debug, Clone, Error)]
pub enum CompactionError {
    #[error("already compacted")]
    AlreadyCompacted,
    #[error("llm error: {0}")]
    LlmError(String),
}
