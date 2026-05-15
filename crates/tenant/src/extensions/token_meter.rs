use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use agent_core::context::TurnEndCtx;
use extensions::Extension;

use crate::registry::TenantRegistry;

/// Per-tenant token usage metering extension.
///
/// Records LLM token consumption on each turn end.
pub struct TenantTokenMeterExtension {
    registry: Arc<TenantRegistry>,
}

impl TenantTokenMeterExtension {
    pub fn new(registry: Arc<TenantRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Extension for TenantTokenMeterExtension {
    fn name(&self) -> &str {
        "tenant-token-meter"
    }

    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        let Some(supervisor) = self.registry.get(&ctx.tenant_id) else {
            warn!(
                tenant_id = %ctx.tenant_id,
                "turn_end from unregistered tenant — ignoring"
            );
            return;
        };

        supervisor.record_usage(&ctx.usage);
    }
}
