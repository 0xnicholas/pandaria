use tenant::{Tenant, TenantQuota, TenantRegistry};

#[test]
fn test_tenant_error_display() {
    let err = tenant::TenantError::TenantNotFound("t1".to_string());
    assert!(err.to_string().contains("t1"));
}

#[test]
fn test_registry_register_and_get() {
    let registry = TenantRegistry::new();
    let tenant = Tenant::new("t1", TenantQuota::default());
    registry.register(tenant.clone()).unwrap();

    let sv = registry.get("t1").unwrap();
    assert_eq!(sv.tenant_id(), "t1");
}

#[test]
fn test_registry_duplicate_rejected() {
    let registry = TenantRegistry::new();
    let tenant = Tenant::new("t1", TenantQuota::default());
    registry.register(tenant.clone()).unwrap();

    let err = registry.register(tenant).unwrap_err();
    assert!(matches!(err, tenant::TenantError::TenantAlreadyExists(_)));
}

#[test]
fn test_registry_unknown_tenant() {
    let registry = TenantRegistry::new();
    assert!(registry.get("unknown").is_none());
}

#[tokio::test]
async fn test_registry_concurrent_sessions() {
    let registry = TenantRegistry::new();
    let tenant = Tenant::new(
        "t1",
        TenantQuota {
            max_concurrent_sessions: 100,
            ..TenantQuota::default()
        },
    );
    registry.register(tenant).unwrap();

    let sv = registry.get("t1").unwrap();

    let handles: Vec<_> = (0..100)
        .map(|_| {
            let sv = sv.clone();
            tokio::spawn(async move { sv.reserve_session() })
        })
        .collect();

    let mut guards = Vec::new();
    for h in handles {
        guards.push(h.await.unwrap().unwrap());
    }

    assert_eq!(sv.quota_status().active_sessions, 100);

    drop(guards);
}

#[test]
fn test_registry_unregister() {
    let registry = TenantRegistry::new();
    let tenant = Tenant::new("t1", TenantQuota::default());
    registry.register(tenant).unwrap();

    assert!(registry.contains("t1"));
    let removed = registry.unregister("t1");
    assert!(removed.is_some());
    assert!(!registry.contains("t1"));
}
