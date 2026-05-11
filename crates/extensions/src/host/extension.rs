use async_trait::async_trait;

use agent_core::context::{
    AgentEndCtx, BeforeAgentStartCtx, CompactCtx, CompactEndCtx, ContextCtx,
    ProviderRequestCtx, ProviderResponseCtx, SessionCtx, ToolCallCtx, ToolExecutionEndCtx,
    ToolExecutionStartCtx, ToolExecutionUpdateCtx, ToolResultCtx, TurnEndCtx,
};
use agent_core::mutations::{
    BeforeAgentStartMutation, CompactDecision, ContextMutation, HookDecision,
    ProviderRequestMutation, ProviderResponseMutation, ToolCallMutation, ToolResultMutation,
};
use agent_core::error::AgentError;
use agent_core::types::AgentToolResult;
use llm_client::ToolDef;

/// Extension trait — the abstract boundary for all extension implementations.
///
/// Each hook method has a default empty implementation. Extensions override
/// only the hooks they need.
#[async_trait]
pub trait Extension: Send + Sync {
    /// Unique extension name. Used for logging, metrics, and routing.
    fn name(&self) -> &str;

    /// Tool definitions this extension contributes to the agent.
    /// Default: no tools.
    fn tools(&self) -> Vec<ToolDef> {
        vec![]
    }

    /// Override the default execution mode for specific tools registered by this extension.
    ///
    /// By default every tool runs in `Parallel` mode.  Return a map of
    /// `tool_name → ToolExecutionMode::Sequential` for tools that must be
    /// executed one-at-a-time (e.g. stateful side-effect tools).
    fn tool_execution_modes(&self) -> std::collections::HashMap<String, agent_core::types::ToolExecutionMode> {
        std::collections::HashMap::new()
    }

    // ═══ Blocking hooks — first-block-wins ═══

    /// Blocking hook with input mutation support.
    /// Returns (decision, mutation). Even when Block is returned,
    /// accumulated mutations from previous handlers are preserved.
    async fn on_tool_call(&self,
        _ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        (HookDecision::Continue, ToolCallMutation::default())
    }

    async fn on_before_compact(&self,
        _ctx: &CompactCtx,
    ) -> CompactDecision {
        CompactDecision::Continue
    }

    // ═══ Chaining hooks — chain merge ═══

    async fn on_tool_result(&self,
        _ctx: &ToolResultCtx,
    ) -> ToolResultMutation {
        ToolResultMutation::default()
    }

    async fn on_context(&self,
        _ctx: &ContextCtx,
    ) -> ContextMutation {
        ContextMutation::default()
    }

    async fn on_before_agent_start(&self,
        _ctx: &BeforeAgentStartCtx,
    ) -> BeforeAgentStartMutation {
        BeforeAgentStartMutation::default()
    }

    async fn on_before_provider_request(&self,
        _ctx: &ProviderRequestCtx,
    ) -> ProviderRequestMutation {
        ProviderRequestMutation::default()
    }

    async fn on_after_provider_response(&self,
        _ctx: &ProviderResponseCtx,
    ) -> ProviderResponseMutation {
        ProviderResponseMutation::default()
    }

    // ═══ Tool execution — runs to completion, no timeout ═══

    /// Execute a tool registered by this extension.
    ///
    /// Called when the LLM invokes a tool whose name matches one of this
    /// extension's `tools()` definitions. Unlike blocking/chain hooks,
    /// tool execution has NO framework-imposed timeout.
    async fn execute_tool(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
        Err(AgentError::ToolExecutionFailed(
            "tool defined but not executable by this extension".into(),
        ))
    }

    // ═══ Observational hooks — fire-and-forget ═══

    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {}
    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {}
    async fn on_session_start(&self, _ctx: &SessionCtx) {}
    async fn on_tool_execution_start(&self, _ctx: &ToolExecutionStartCtx) {}
    async fn on_tool_execution_update(&self, _ctx: &ToolExecutionUpdateCtx) {}
    async fn on_tool_execution_end(&self, _ctx: &ToolExecutionEndCtx) {}
    async fn on_compact_end(&self, _ctx: &CompactEndCtx) {}
}