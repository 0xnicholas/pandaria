use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::context::TenantContext;
use crate::error::TenantError;
use crate::meter::SlidingWindowMeter;
use crate::tenant::{QuotaCheck, Tenant, TenantQuota};

/// Per-tenant resource supervisor. Tracks active sessions and usage meters.
pub struct TenantSupervisor {
    tenant_id: String,
    quota: TenantQuota,
    active_sessions: AtomicUsize,
    token_meter: SlidingWindowMeter,     // 24h window
    tool_call_meter: SlidingWindowMeter, // 1min window
    cpu_time_meter: SlidingWindowMeter,  // 24h window
}

/// Snapshot of current quota consumption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaStatus {
    pub tenant_id: String,
    pub active_sessions: u32,
    pub tokens_consumed: u64,
    pub tool_calls_in_window: usize,
    pub cpu_time_ms_consumed: u64,
}

/// RAII guard for a reserved session slot. Auto-releases on drop.
pub struct SessionGuard {
    supervisor: Arc<TenantSupervisor>,
    released: bool,
}

impl SessionGuard {
    /// Explicitly release the session slot before drop.
    pub fn release(mut self) {
        self.supervisor.release_session();
        self.released = true;
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if !self.released {
            self.supervisor.release_session();
        }
    }
}

impl TenantSupervisor {
    pub fn new(tenant: Tenant) -> Self {
        Self {
            tenant_id: tenant.id,
            quota: tenant.quota,
            active_sessions: AtomicUsize::new(0),
            token_meter: SlidingWindowMeter::new(Duration::from_secs(86400)),
            tool_call_meter: SlidingWindowMeter::new(Duration::from_secs(60)),
            cpu_time_meter: SlidingWindowMeter::new(Duration::from_secs(86400)),
        }
    }

    /// Create supervisor from Aspectus TenantContext.
    pub fn from_context(ctx: &TenantContext) -> Self {
        Self {
            tenant_id: ctx.tenant_id.clone(),
            quota: ctx.quotas,
            active_sessions: AtomicUsize::new(0),
            token_meter: SlidingWindowMeter::new(Duration::from_secs(86400)),
            tool_call_meter: SlidingWindowMeter::new(Duration::from_secs(60)),
            cpu_time_meter: SlidingWindowMeter::new(Duration::from_secs(86400)),
        }
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// Attempt to reserve a session slot. Returns a `SessionGuard` that auto-releases on drop.
    /// Fails if at capacity.
    pub fn reserve_session(self: &Arc<Self>) -> Result<SessionGuard, TenantError> {
        let current = self.active_sessions.fetch_add(1, Ordering::SeqCst) + 1;
        if current > self.quota.max_concurrent_sessions as usize {
            self.active_sessions.fetch_sub(1, Ordering::SeqCst);
            return Err(TenantError::SessionLimitExceeded {
                tenant_id: self.tenant_id.clone(),
                max: self.quota.max_concurrent_sessions,
                current: (current - 1) as u32,
            });
        }
        Ok(SessionGuard {
            supervisor: self.clone(),
            released: false,
        })
    }

    /// Release a session slot. Called automatically by `SessionGuard::drop`.
    fn release_session(&self) {
        let prev = self.active_sessions.fetch_sub(1, Ordering::SeqCst);
        if prev == 0 {
            // Underflow protection — reset to 0
            self.active_sessions.store(0, Ordering::SeqCst);
        }
    }

    /// Record LLM token usage.
    pub fn record_usage(&self, usage: &ai_provider::Usage) {
        self.token_meter.record(usage.total_tokens);
    }

    /// Record a tool call.
    pub fn record_tool_call(&self) {
        self.tool_call_meter.record(1);
    }

    /// Record CPU time (wall-clock proxy in ms).
    pub fn record_cpu_time_ms(&self, ms: u64) {
        self.cpu_time_meter.record(ms);
    }

    /// Check whether a quota operation is allowed.
    pub fn check_quota(&self, check: QuotaCheck) -> Result<(), TenantError> {
        match check {
            QuotaCheck::SessionCreation => {
                let current = self.active_sessions.load(Ordering::SeqCst) as u32;
                if current >= self.quota.max_concurrent_sessions {
                    return Err(TenantError::SessionLimitExceeded {
                        tenant_id: self.tenant_id.clone(),
                        max: self.quota.max_concurrent_sessions,
                        current,
                    });
                }
            }
            QuotaCheck::ToolCall => {
                let calls = self.tool_call_meter.count();
                if calls >= self.quota.max_tool_calls_per_minute as usize {
                    return Err(TenantError::ToolCallRateLimitExceeded {
                        tenant_id: self.tenant_id.clone(),
                        calls,
                    });
                }
            }
            QuotaCheck::TokenUsage { input, output } => {
                let total = self.token_meter.sum() + input + output;
                if total > self.quota.max_tokens_per_day {
                    return Err(TenantError::TokenBudgetExceeded {
                        tenant_id: self.tenant_id.clone(),
                        consumed: self.token_meter.sum(),
                        budget: self.quota.max_tokens_per_day,
                    });
                }
            }
        }
        Ok(())
    }

    /// Get current quota consumption snapshot.
    pub fn quota_status(&self) -> QuotaStatus {
        QuotaStatus {
            tenant_id: self.tenant_id.clone(),
            active_sessions: self.active_sessions.load(Ordering::SeqCst) as u32,
            tokens_consumed: self.token_meter.sum(),
            tool_calls_in_window: self.tool_call_meter.count(),
            cpu_time_ms_consumed: self.cpu_time_meter.sum(),
        }
    }

    /// Return the tenant's quota configuration.
    pub fn quota(&self) -> &TenantQuota {
        &self.quota
    }

    /// Return the number of currently active sessions.
    pub fn active_session_count(&self) -> usize {
        self.active_sessions.load(Ordering::SeqCst)
    }

    /// Return the maximum allowed concurrent sessions.
    pub fn max_concurrent_sessions(&self) -> usize {
        self.quota.max_concurrent_sessions as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_context_creates_supervisor_with_correct_fields() {
        let ctx = TenantContext {
            tenant_id: "acme".into(),
            user_id: Some("user-1".into()),
            scopes: vec!["pandaria:session:create".into()],
            quotas: TenantQuota {
                max_concurrent_sessions: 20,
                max_tokens_per_day: 2_000_000,
                max_tool_calls_per_minute: 100,
                cpu_time_budget_ms_per_day: 7_200_000,
            },
            cached_at: std::time::Instant::now(),
        };

        let supervisor = TenantSupervisor::from_context(&ctx);

        assert_eq!(supervisor.tenant_id(), "acme");
        assert_eq!(supervisor.quota().max_concurrent_sessions, 20);
        assert_eq!(supervisor.quota().max_tokens_per_day, 2_000_000);
        assert_eq!(supervisor.quota().max_tool_calls_per_minute, 100);
        assert_eq!(supervisor.quota().cpu_time_budget_ms_per_day, 7_200_000);
        assert_eq!(supervisor.active_session_count(), 0);
    }

    #[test]
    fn from_context_and_new_produce_equivalent_supervisors() {
        let tenant = Tenant::new(
            "acme",
            TenantQuota {
                max_concurrent_sessions: 10,
                max_tokens_per_day: 500_000,
                max_tool_calls_per_minute: 30,
                cpu_time_budget_ms_per_day: 1_800_000,
            },
        );

        let from_new = TenantSupervisor::new(tenant.clone());

        let ctx = TenantContext {
            tenant_id: tenant.id.clone(),
            user_id: None,
            scopes: vec![],
            quotas: tenant.quota,
            cached_at: std::time::Instant::now(),
        };

        let from_ctx = TenantSupervisor::from_context(&ctx);

        assert_eq!(from_new.tenant_id(), from_ctx.tenant_id());
        assert_eq!(from_new.quota(), from_ctx.quota());
        assert_eq!(from_new.active_session_count(), from_ctx.active_session_count());
    }
}
