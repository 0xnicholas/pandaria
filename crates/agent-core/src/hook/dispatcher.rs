use async_trait::async_trait;

use crate::hook::context::{
    AgentEndCtx, BeforeAgentStartCtx, CompactCtx, CompactEndCtx, ContextCtx, ProviderRequestCtx,
    ProviderResponseCtx, SessionCtx, ToolCallCtx, ToolExecutionEndCtx, ToolExecutionStartCtx,
    ToolResultCtx, TurnEndCtx,
};
use crate::hook::mutations::{
    BeforeAgentStartMutation, CompactDecision, ContextMutation, HookDecision,
    ProviderRequestMutation, ProviderResponseMutation, ToolCallMutation, ToolResultMutation,
};

/// Dependency-inversion boundary for extension hook dispatch.
///
/// Blocking hooks (`on_tool_call`, `on_before_compact`) follow first-block-wins semantics.
/// Chaining hooks (`on_tool_result`, `on_context`, `on_before_agent_start`,
/// `on_before_provider_request`, `on_after_provider_response`) chain-merge mutations.
/// Observational hooks are fire-and-forget.
#[async_trait]
pub trait HookDispatcher: Send + Sync {
    // ═══ Blocking hooks — first-block-wins ═══

    /// Blocking hook with input mutation support.
    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        (HookDecision::Continue, ToolCallMutation::default())
    }

    /// Blocking hook for compaction.
    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Continue
    }

    // ═══ Chaining hooks — chain merge ═══

    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        ToolResultMutation::default()
    }

    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
        ContextMutation::default()
    }

    async fn on_before_agent_start(&self, _ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
        BeforeAgentStartMutation::default()
    }

    async fn on_before_provider_request(
        &self,
        _ctx: &ProviderRequestCtx,
    ) -> ProviderRequestMutation {
        ProviderRequestMutation::default()
    }

    async fn on_after_provider_response(
        &self,
        _ctx: &ProviderResponseCtx,
    ) -> ProviderResponseMutation {
        ProviderResponseMutation::default()
    }

    // ═══ Observational hooks — fire-and-forget ═══

    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {}
    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {}
    async fn on_session_start(&self, _ctx: &SessionCtx) {}
    async fn on_tool_execution_start(&self, _ctx: &ToolExecutionStartCtx) {}
    async fn on_tool_execution_end(&self, _ctx: &ToolExecutionEndCtx) {}
    async fn on_compact_end(&self, _ctx: &CompactEndCtx) {}
}
