use async_trait::async_trait;

use agent_core::context::{ToolCallCtx, ToolResultCtx, TurnEndCtx};
use agent_core::mutations::{HookDecision, ToolCallMutation, ToolResultMutation};

use crate::host::extension::Extension;

/// Audit extension — records all tool calls and turn events to tracing journal.
///
/// Never blocks, never mutates.
pub struct AuditExtension;

#[async_trait]
impl Extension for AuditExtension {
    fn name(&self) -> &str {
        "audit"
    }

    async fn on_tool_call(
        &self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        tracing::info!(
            target: "pandaria.audit",
            tool_name = %ctx.tool_name,
            tool_call_id = %ctx.tool_call_id,
            tenant_id = %ctx.tenant_id,
            session_id = %ctx.session_id,
            action = "tool_call_start"
        );
        (HookDecision::Continue, ToolCallMutation::default())
    }

    async fn on_tool_result(
        &self,
        ctx: &ToolResultCtx,
    ) -> ToolResultMutation {
        tracing::info!(
            target: "pandaria.audit",
            tool_name = %ctx.tool_name,
            tool_call_id = %ctx.tool_call_id,
            tenant_id = %ctx.tenant_id,
            session_id = %ctx.session_id,
            is_error = ctx.is_error,
            action = "tool_call_end"
        );
        ToolResultMutation::default()
    }

    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        tracing::info!(
            target: "pandaria.audit",
            turn_index = ctx.turn_index,
            message_count = ctx.messages.len(),
            tenant_id = %ctx.tenant_id,
            session_id = %ctx.session_id,
            action = "turn_end"
        );
    }
}