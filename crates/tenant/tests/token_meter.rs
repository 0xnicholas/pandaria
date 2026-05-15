use std::sync::Arc;

use agent_core::context::TurnEndCtx;
use extensions::Extension;
use tenant::extensions::token_meter::TenantTokenMeterExtension;
use tenant::{Tenant, TenantQuota, TenantRegistry};

#[tokio::test]
async fn test_token_meter_records_usage() {
    let registry = Arc::new(TenantRegistry::new());
    let tenant = Tenant::new("t1", TenantQuota::default());
    registry.register(tenant).unwrap();

    let ext = TenantTokenMeterExtension::new(registry.clone());
    let ctx = TurnEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        turn_index: 0,
        messages: vec![],
        usage: ai_provider::Usage {
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
    };

    ext.on_turn_end(&ctx).await;

    let sv = registry.get("t1").unwrap();
    let status = sv.quota_status();
    assert_eq!(status.tokens_consumed, 30);
}

#[tokio::test]
async fn test_token_meter_unknown_tenant_ignored() {
    let registry = Arc::new(TenantRegistry::new());
    let ext = TenantTokenMeterExtension::new(registry.clone());
    let ctx = TurnEndCtx {
        tenant_id: "unknown".to_string(),
        session_id: "s1".to_string(),
        turn_index: 0,
        messages: vec![],
        usage: ai_provider::Usage {
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
    };

    // Should not panic
    ext.on_turn_end(&ctx).await;

    // No tenant registered, so nothing to check
    assert!(registry.get("unknown").is_none());
}
