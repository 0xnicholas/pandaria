use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum TenantError {
    #[error("tenant not found: {0}")]
    TenantNotFound(String),

    #[error("tenant already registered: {0}")]
    TenantAlreadyExists(String),

    #[error("session limit exceeded for tenant {tenant_id}: max {max}, current {current}")]
    SessionLimitExceeded {
        tenant_id: String,
        max: u32,
        current: u32,
    },

    #[error("token budget exceeded for tenant {tenant_id}: consumed {consumed}, budget {budget}")]
    TokenBudgetExceeded {
        tenant_id: String,
        consumed: u64,
        budget: u64,
    },

    #[error("tool call rate limit exceeded for tenant {tenant_id}: {calls} calls in window")]
    ToolCallRateLimitExceeded {
        tenant_id: String,
        calls: usize,
    },

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("internal error: {message}")]
    Internal { tenant_id: String, message: String },
}
