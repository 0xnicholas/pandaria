use std::sync::Arc;
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

#[test]
fn resolve_or_insert_creates_new_supervisor() {
    let registry = TenantRegistry::new();
    let ctx = make_context("acme", 10);

    let sv = registry.resolve_or_insert(&ctx).unwrap();
    assert_eq!(sv.tenant_id(), "acme");
    assert_eq!(sv.max_concurrent_sessions(), 10);
}

#[test]
fn resolve_or_insert_returns_cached_when_not_stale() {
    let registry = TenantRegistry::new();
    let ctx = make_context("acme", 10);

    let sv1 = registry.resolve_or_insert(&ctx).unwrap();
    let sv2 = registry.resolve_or_insert(&ctx).unwrap();

    // Same Arc pointer returned (cached)
    assert!(Arc::ptr_eq(&sv1, &sv2));
}

#[test]
fn resolve_or_insert_refreshes_when_stale() {
    // Use a 0-second TTL so every call is stale
    let registry = TenantRegistry::with_ttl(std::time::Duration::ZERO);
    let ctx = make_context("acme", 10);

    let sv1 = registry.resolve_or_insert(&ctx).unwrap();
    // Small sleep to ensure Instant difference
    std::thread::sleep(std::time::Duration::from_millis(1));
    let sv2 = registry.resolve_or_insert(&ctx).unwrap();

    // Different Arc pointers (refreshed)
    assert!(!Arc::ptr_eq(&sv1, &sv2));
}

#[test]
fn refresh_replaces_supervisor() {
    let registry = TenantRegistry::new();
    let ctx = make_context("acme", 10);

    let sv1 = registry.resolve_or_insert(&ctx).unwrap();
    let sv2 = registry.refresh(&ctx).unwrap();

    // Different Arc pointers after refresh
    assert!(!Arc::ptr_eq(&sv1, &sv2));
    assert_eq!(sv2.tenant_id(), "acme");
}

#[test]
fn evict_removes_and_returns_supervisor() {
    let registry = TenantRegistry::new();
    let ctx = make_context("acme", 10);

    registry.resolve_or_insert(&ctx).unwrap();
    assert!(registry.contains("acme"));

    let evicted = registry.evict("acme");
    assert!(evicted.is_some());
    assert_eq!(evicted.unwrap().tenant_id(), "acme");
    assert!(!registry.contains("acme"));
}

#[test]
fn evict_unknown_tenant_returns_none() {
    let registry = TenantRegistry::new();
    assert!(registry.evict("unknown").is_none());
}

/// Helper to create a TenantContext for testing.
fn make_context(tenant_id: &str, max_sessions: u32) -> tenant::TenantContext {
    use std::time::Instant;
    tenant::TenantContext {
        tenant_id: tenant_id.to_string(),
        user_id: None,
        scopes: vec![],
        quotas: tenant::TenantQuota {
            max_concurrent_sessions: max_sessions,
            ..tenant::TenantQuota::default()
        },
        cached_at: Instant::now(),
    }
}
