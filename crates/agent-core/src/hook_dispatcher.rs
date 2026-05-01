use async_trait::async_trait;

use crate::context::{AgentEndCtx, SessionCtx, ToolCallCtx, ToolResultCtx, TurnEndCtx};
use crate::mutations::{HookDecision, ToolResultMutation};
use crate::types::AgentMessage;

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

    /// Chaining hook — each handler transforms messages
    async fn on_context(&self, messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
        messages
    }

    /// Observational hook — fire-and-forget
    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {}

    /// Observational hook — fire-and-forget
    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {}

    /// Observational hook — fire-and-forget
    async fn on_session_start(&self, _ctx: &SessionCtx) {}
}
