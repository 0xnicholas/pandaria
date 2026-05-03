use async_trait::async_trait;

use crate::context::{AgentEndCtx, ContextCtx, SessionCtx, ToolCallCtx, ToolResultCtx, TurnEndCtx};
use crate::mutations::{ContextMutation, HookDecision, ToolResultMutation};

/// Dependency-inversion boundary for extension hook dispatch.
///
/// Blocking hooks (`on_tool_call`) follow first-block-wins semantics.
/// Chaining hooks (`on_tool_result`, `on_context`) chain-merge mutations.
/// Observational hooks (`on_turn_end`, `on_agent_end`, `on_session_start`) are fire-and-forget.
#[async_trait]
pub trait HookDispatcher: Send + Sync {
    /// Blocking hook — first-block-wins
    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> HookDecision {
        HookDecision::Continue
    }

    /// Chaining hook — each handler sees previous mutations
    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        ToolResultMutation::default()
    }

    /// Chaining hook — each handler transforms context messages
    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
        ContextMutation::default()
    }

    /// Observational hook — fire-and-forget
    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {}

    /// Observational hook — fire-and-forget
    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {}

    /// Observational hook — fire-and-forget
    async fn on_session_start(&self, _ctx: &SessionCtx) {}
}
