/// Resource quota configuration for a single tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantQuota {
    /// Maximum number of concurrently active sessions.
    pub max_concurrent_sessions: u32,
    /// Maximum LLM tokens (input + output) per day.
    pub max_tokens_per_day: u64,
    /// Maximum tool calls per minute.
    pub max_tool_calls_per_minute: u32,
    /// CPU time budget in milliseconds per day (wall-clock proxy).
    pub cpu_time_budget_ms_per_day: u64,
}

impl Default for TenantQuota {
    fn default() -> Self {
        Self {
            max_concurrent_sessions: 10,
            max_tokens_per_day: 1_000_000,
            max_tool_calls_per_minute: 60,
            cpu_time_budget_ms_per_day: 3_600_000, // 1 hour
        }
    }
}

/// A registered tenant with its quota.
#[derive(Debug, Clone)]
pub struct Tenant {
    pub id: String,
    pub quota: TenantQuota,
}

impl Tenant {
    pub fn new(id: impl Into<String>, quota: TenantQuota) -> Self {
        Self {
            id: id.into(),
            quota,
        }
    }
}

/// Type of quota check requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaCheck {
    SessionCreation,
    ToolCall,
    TokenUsage { input: u64, output: u64 },
}
