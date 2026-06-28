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

impl TenantQuota {
    /// Parse quota from Aspectus quotas.pandaria JSON value.
    ///
    /// Returns `InvalidQuotasFormat` if value is not a JSON object.
    /// Missing individual fields silently use reasonable defaults.
    pub fn from_aspectus_quotas(
        quotas: &serde_json::Value,
    ) -> Result<Self, crate::error::TenantError> {
        use crate::error::TenantError;

        let obj = quotas.as_object().ok_or_else(|| {
            TenantError::InvalidQuotasFormat("quotas.pandaria must be a JSON object".into())
        })?;

        Ok(Self {
            max_concurrent_sessions: extract_u32(obj, "max_concurrent_sessions", 10),
            max_tokens_per_day: extract_u64(obj, "max_tokens_per_day", 1_000_000),
            max_tool_calls_per_minute: extract_u32(obj, "max_tool_calls_per_minute", 60),
            cpu_time_budget_ms_per_day: extract_u64(
                obj,
                "cpu_time_budget_ms_per_day",
                3_600_000,
            ),
        })
    }
}

fn extract_u32(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: u32,
) -> u32 {
    obj.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(default)
}

fn extract_u64(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: u64,
) -> u64 {
    obj.get(key).and_then(|v| v.as_u64()).unwrap_or(default)
}

/// Legacy default implementation. Prefer [`TenantQuota::from_aspectus_quotas`] for new code.
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
    CpuBudget,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TenantError;

    #[test]
    fn from_aspectus_quotas_full_valid() {
        let json = serde_json::json!({
            "max_concurrent_sessions": 50,
            "max_tokens_per_day": 5_000_000,
            "max_tool_calls_per_minute": 120,
            "cpu_time_budget_ms_per_day": 7_200_000
        });
        let quota = TenantQuota::from_aspectus_quotas(&json).unwrap();
        assert_eq!(quota.max_concurrent_sessions, 50);
        assert_eq!(quota.max_tokens_per_day, 5_000_000);
        assert_eq!(quota.max_tool_calls_per_minute, 120);
        assert_eq!(quota.cpu_time_budget_ms_per_day, 7_200_000);
    }

    #[test]
    fn from_aspectus_quotas_partial_json() {
        let json = serde_json::json!({
            "max_concurrent_sessions": 5
        });
        let quota = TenantQuota::from_aspectus_quotas(&json).unwrap();
        assert_eq!(quota.max_concurrent_sessions, 5);
        // Missing fields use defaults
        assert_eq!(quota.max_tokens_per_day, 1_000_000);
        assert_eq!(quota.max_tool_calls_per_minute, 60);
        assert_eq!(quota.cpu_time_budget_ms_per_day, 3_600_000);
    }

    #[test]
    fn from_aspectus_quotas_empty_object() {
        let json = serde_json::json!({});
        let quota = TenantQuota::from_aspectus_quotas(&json).unwrap();
        // All defaults
        assert_eq!(quota.max_concurrent_sessions, 10);
        assert_eq!(quota.max_tokens_per_day, 1_000_000);
        assert_eq!(quota.max_tool_calls_per_minute, 60);
        assert_eq!(quota.cpu_time_budget_ms_per_day, 3_600_000);
    }

    #[test]
    fn from_aspectus_quotas_invalid_array() {
        let json = serde_json::json!([1, 2, 3]);
        let err = TenantQuota::from_aspectus_quotas(&json).unwrap_err();
        assert!(matches!(err, TenantError::InvalidQuotasFormat(_)));
    }

    #[test]
    fn from_aspectus_quotas_invalid_string() {
        let json = serde_json::json!("not an object");
        let err = TenantQuota::from_aspectus_quotas(&json).unwrap_err();
        assert!(matches!(err, TenantError::InvalidQuotasFormat(_)));
    }

    #[test]
    fn from_aspectus_quotas_invalid_number_field() {
        // String where number expected → uses default
        let json = serde_json::json!({
            "max_concurrent_sessions": "not a number"
        });
        let quota = TenantQuota::from_aspectus_quotas(&json).unwrap();
        assert_eq!(quota.max_concurrent_sessions, 10); // default
    }

    #[test]
    fn default_still_works() {
        let quota = TenantQuota::default();
        assert_eq!(quota.max_concurrent_sessions, 10);
    }
}
