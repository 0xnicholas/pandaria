use std::sync::Arc;

use agent_core::context::ToolCallCtx;
use agent_core::mutations::HookDecision;
use extensions::Extension;
use tenant::extensions::quota::TenantQuotaExtension;
use tenant::{Tenant, TenantQuota, TenantRegistry};

#[tokio::test]
async fn test_quota_extension_allows_within_limit() {
    let registry = Arc::new(TenantRegistry::new());
    let tenant = Tenant::new("t1", TenantQuota {
        max_tool_calls_per_minute: 5,
        ..TenantQuota::default()
    });
    registry.register(tenant).unwrap();

    let ext = TenantQuotaExtension::new(registry);
    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    // First 5 calls should pass
    for _ in 0..5 {
        let (decision, _) = ext.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Continue));
    }

    // 6th should be blocked
    let (decision, _) = ext.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
}

#[tokio::test]
async fn test_quota_extension_unknown_tenant() {
    let registry = Arc::new(TenantRegistry::new());
    let ext = TenantQuotaExtension::new(registry);
    let ctx = ToolCallCtx {
        tenant_id: "unknown".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    // Unknown tenant: blocked by default
    let (decision, _) = ext.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
}

#[tokio::test]
async fn test_quota_extension_allow_unknown() {
    let registry = Arc::new(TenantRegistry::new());
    let ext = TenantQuotaExtension::new(registry).with_allow_unknown(true);
    let ctx = ToolCallCtx {
        tenant_id: "unknown".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    // Unknown tenant allowed in dev mode
    let (decision, _) = ext.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));
}
