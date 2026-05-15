use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use agent_core::context::ToolCallCtx;
use agent_core::mutations::{HookDecision, ToolCallMutation};
use extensions::Extension;

use crate::registry::TenantRegistry;
use crate::tenant::QuotaCheck;

/// Per-tenant quota enforcement extension.
///
/// Checks tool-call rate limits per tenant.
/// Unknown tenants are blocked by default (`allow_unknown = false`).
pub struct TenantQuotaExtension {
    registry: Arc<TenantRegistry>,
    allow_unknown: bool,
}

impl TenantQuotaExtension {
    /// Create extension. `allow_unknown` controls behavior for unregistered tenants.
    /// Default: `false` (block unknown tenants).
    pub fn new(registry: Arc<TenantRegistry>) -> Self {
        Self {
            registry,
            allow_unknown: false,
        }
    }

    /// Opt-in to allow unregistered tenants (e.g., for dev mode).
    pub fn with_allow_unknown(mut self, allow: bool) -> Self {
        self.allow_unknown = allow;
        self
    }
}

#[async_trait]
impl Extension for TenantQuotaExtension {
    fn name(&self) -> &str {
        "tenant-quota"
    }

    async fn on_tool_call(
        &self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        let Some(supervisor) = self.registry.get(&ctx.tenant_id) else {
            if self.allow_unknown {
                warn!(
                    tenant_id = %ctx.tenant_id,
                    "tool call from unregistered tenant — allowing (dev mode)"
                );
                return (HookDecision::Continue, ToolCallMutation::default());
            }
            return (
                HookDecision::Block {
                    reason: format!("tenant '{}' is not registered", ctx.tenant_id),
                },
                ToolCallMutation::default(),
            );
        };

        // Check tool call rate limit
        if let Err(e) = supervisor.check_quota(QuotaCheck::ToolCall) {
            return (
                HookDecision::Block { reason: e.to_string() },
                ToolCallMutation::default(),
            );
        }

        supervisor.record_tool_call();
        (HookDecision::Continue, ToolCallMutation::default())
    }
}
