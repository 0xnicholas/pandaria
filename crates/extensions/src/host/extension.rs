use async_trait::async_trait;

use agent_core::context::{AgentEndCtx, ContextCtx, SessionCtx, ToolCallCtx, ToolResultCtx, TurnEndCtx};
use agent_core::mutations::{ContextMutation, HookDecision, ToolResultMutation};
use llm_client::ToolDef;

#[async_trait]
pub trait Extension: Send + Sync {
    fn name(&self) -> &str;

    fn tools(&self) -> Vec<ToolDef> {
        vec![]
    }

    /// Blocking hook — first-block-wins
    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> HookDecision {
        HookDecision::Continue
    }

    /// Chaining hook — chain merge (each handler sees previous mutations)
    async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
        ToolResultMutation::default()
    }

    /// Chaining hook — chain merge (each handler transforms messages)
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
