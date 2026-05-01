use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("tool not found: {0}")]
    ToolNotFound(String),

    #[error("tool execution failed: {0}")]
    ToolExecutionFailed(String),

    #[error("hook dispatch error: {0}")]
    HookDispatchError(String),

    #[error("llm error: {0}")]
    LlmError(#[from] llm_client::LlmError),

    #[error("cancelled")]
    Cancelled,
}
