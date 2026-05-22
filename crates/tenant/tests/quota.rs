use tenant::{Tenant, TenantQuota};

#[test]
fn test_tenant_quota_defaults() {
    let quota = TenantQuota::default();
    assert_eq!(quota.max_concurrent_sessions, 10);
    assert_eq!(quota.max_tokens_per_day, 1_000_000);
    assert_eq!(quota.max_tool_calls_per_minute, 60);
    assert_eq!(quota.cpu_time_budget_ms_per_day, 3_600_000);
}

#[test]
fn test_tenant_creation() {
    let tenant = Tenant::new("t1", TenantQuota::default());
    assert_eq!(tenant.id, "t1");
}

#[test]
fn test_supervisor_tool_call_rate_limit() {
    use tenant::{QuotaCheck, TenantError, TenantSupervisor};

    let tenant = Tenant::new(
        "t1",
        TenantQuota {
            max_tool_calls_per_minute: 3,
            ..TenantQuota::default()
        },
    );
    let supervisor = TenantSupervisor::new(tenant);

    // 3 calls within limit
    for _ in 0..3 {
        supervisor.check_quota(QuotaCheck::ToolCall).unwrap();
        supervisor.record_tool_call();
    }

    // 4th should fail
    let err = supervisor.check_quota(QuotaCheck::ToolCall).unwrap_err();
    assert!(matches!(err, TenantError::ToolCallRateLimitExceeded { .. }));
}

#[test]
fn test_supervisor_session_creation_quota() {
    use std::sync::Arc;
    use tenant::{QuotaCheck, TenantError, TenantSupervisor};

    let tenant = Tenant::new(
        "t1",
        TenantQuota {
            max_concurrent_sessions: 1,
            ..TenantQuota::default()
        },
    );
    let supervisor = Arc::new(TenantSupervisor::new(tenant));

    let _guard = supervisor.reserve_session().unwrap();

    // Another session creation should fail at quota check
    let err = supervisor
        .check_quota(QuotaCheck::SessionCreation)
        .unwrap_err();
    assert!(matches!(err, TenantError::SessionLimitExceeded { .. }));
}
