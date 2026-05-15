use std::time::Duration;

use tenant::{Tenant, TenantQuota, TenantSupervisor, QuotaCheck};
use tenant::meter::SlidingWindowMeter;

#[test]
fn test_sliding_window_count() {
    let meter = SlidingWindowMeter::new(Duration::from_secs(60));
    meter.record(100);
    meter.record(200);
    assert_eq!(meter.sum(), 300);
    assert_eq!(meter.count(), 2);
}

#[tokio::test]
async fn test_sliding_window_expiration() {
    let meter = SlidingWindowMeter::new(Duration::from_millis(50));
    meter.record(100);
    assert_eq!(meter.sum(), 100);
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(meter.sum(), 0);
}

#[test]
fn test_supervisor_session_tracking() {
    use std::sync::Arc;
    use tenant::TenantError;

    let tenant = Tenant::new("t1", TenantQuota {
        max_concurrent_sessions: 2,
        ..TenantQuota::default()
    });
    let supervisor = Arc::new(TenantSupervisor::new(tenant));

    // Reserve 2 sessions
    let guard1 = supervisor.reserve_session().unwrap();
    let guard2 = supervisor.reserve_session().unwrap();

    // 3rd should fail
    let err = match supervisor.reserve_session() {
        Err(e) => e,
        Ok(_) => panic!("expected error"),
    };
    assert!(matches!(err, TenantError::SessionLimitExceeded { .. }));

    // Drop one guard, then reserve should succeed
    drop(guard1);
    let _guard3 = supervisor.reserve_session().unwrap();

    drop(guard2);
    drop(_guard3);
    assert_eq!(supervisor.quota_status().active_sessions, 0);
}

#[test]
fn test_supervisor_token_metering() {
    use std::sync::Arc;
    use tenant::TenantError;

    let tenant = Tenant::new("t1", TenantQuota {
        max_tokens_per_day: 100,
        ..TenantQuota::default()
    });
    let supervisor = Arc::new(TenantSupervisor::new(tenant));

    supervisor.record_usage(&ai_provider::Usage {
        input_tokens: 30,
        output_tokens: 20,
        total_tokens: 50,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });

    let status = supervisor.quota_status();
    assert_eq!(status.tokens_consumed, 50);

    // Exceed budget
    let err = supervisor.check_quota(QuotaCheck::TokenUsage { input: 30, output: 30 }).unwrap_err();
    assert!(matches!(err, TenantError::TokenBudgetExceeded { .. }));
}

#[test]
fn test_supervisor_quota_status() {
    let tenant = Tenant::new("t1", TenantQuota::default());
    let supervisor = TenantSupervisor::new(tenant);

    let status = supervisor.quota_status();
    assert_eq!(status.tenant_id, "t1");
    assert_eq!(status.active_sessions, 0);
    assert_eq!(status.tokens_consumed, 0);
    assert_eq!(status.tool_calls_in_window, 0);
    assert_eq!(status.cpu_time_ms_consumed, 0);
}
