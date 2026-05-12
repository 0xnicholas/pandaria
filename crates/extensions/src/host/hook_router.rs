use std::sync::Arc;

use async_trait::async_trait;

use agent_core::context::{
    AgentEndCtx, BeforeAgentStartCtx, CompactCtx, CompactEndCtx, ContextCtx,
    ProviderRequestCtx, ProviderResponseCtx, SessionCtx, ToolCallCtx, ToolExecutionEndCtx,
    ToolExecutionStartCtx, ToolResultCtx, TurnEndCtx,
};
use agent_core::mutations::{
    BeforeAgentStartMutation, CompactDecision, ContextMutation, HookDecision,
    ProviderRequestMutation, ProviderResponseMutation, ToolCallMutation, ToolResultMutation,
};

use super::event_bus::EventBus;
use super::extension_actor::{ExtensionHandle, ObsEvent};

/// Implements agent_core::HookDispatcher by routing to ExtensionActors.
///
/// - Blocking hooks (on_tool_call, on_before_compact): serial dispatch, first-block-wins
/// - Chaining hooks (on_tool_result, on_context, on_before_agent_start,
///   on_before_provider_request, on_after_provider_response): serial dispatch, chain merge
/// - Observational hooks: broadcast via EventBus
pub struct HookRouter {
    handles: Vec<ExtensionHandle>,
    event_bus: Arc<EventBus<ObsEvent>>,
}

impl HookRouter {
    pub fn new(handles: Vec<ExtensionHandle>, event_bus: Arc<EventBus<ObsEvent>>) -> Self {
        Self {
            handles,
            event_bus,
        }
    }
}

#[async_trait]
impl agent_core::HookDispatcher for HookRouter {
    // ═══ Blocking hooks — first-block-wins ═══

    async fn on_tool_call(&self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        let mut current_ctx = ctx.clone();
        for handle in &self.handles {
            tracing::debug!(extension = %handle.name, hook = "on_tool_call", "dispatching hook");
            let (decision, mutation) = handle.on_tool_call(current_ctx.clone()).await;

            // Apply mutation to current_ctx so subsequent handlers see sanitized input
            if let Some(input) = mutation.input {
                current_ctx.input = input;
            }

            match decision {
                HookDecision::Block { reason } => {
                    return (
                        HookDecision::Block { reason },
                        ToolCallMutation { input: Some(current_ctx.input) },
                    );
                }
                HookDecision::Continue => continue,
            }
        }
        (
            HookDecision::Continue,
            ToolCallMutation { input: Some(current_ctx.input) },
        )
    }

    async fn on_before_compact(
        &self,
        ctx: &CompactCtx,
    ) -> CompactDecision {
        for handle in &self.handles {
            tracing::debug!(extension = %handle.name, hook = "on_before_compact", "dispatching hook");
            match handle.on_before_compact(ctx.clone()).await {
                CompactDecision::Block { reason } => return CompactDecision::Block { reason },
                CompactDecision::Replace { result } => return CompactDecision::Replace { result },
                CompactDecision::Continue => continue,
            }
        }
        CompactDecision::Continue
    }

    // ═══ Chaining hooks — chain merge ═══

    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
        let mut current_ctx = ctx.clone();
        let mut final_mutation = ToolResultMutation::default();

        for handle in &self.handles {
            tracing::debug!(extension = %handle.name, hook = "on_tool_result", "dispatching hook");
            let mutation = handle.on_tool_result(current_ctx.clone()).await;

            if let Some(ref content) = mutation.content {
                current_ctx.content = content.clone();
                final_mutation.content = Some(content.clone());
            }
            if let Some(ref details) = mutation.details {
                current_ctx.details = Some(details.clone());
                final_mutation.details = Some(details.clone());
            }
            if let Some(is_error) = mutation.is_error {
                current_ctx.is_error = is_error;
                final_mutation.is_error = Some(is_error);
            }
            if let Some(terminate) = mutation.terminate {
                final_mutation.terminate = Some(terminate);
            }
        }

        final_mutation
    }

    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        let original_messages = ctx.messages.clone();
        let mut current_messages = original_messages.clone();

        for handle in &self.handles {
            tracing::debug!(extension = %handle.name, hook = "on_context", "dispatching hook");
            let ctx = ContextCtx {
                tenant_id: ctx.tenant_id.clone(),
                session_id: ctx.session_id.clone(),
                messages: current_messages.clone(),
            };
            let mutation = handle.on_context(ctx).await;
            if let Some(msgs) = mutation.messages {
                current_messages = msgs;
            }
        }

        if current_messages == original_messages {
            ContextMutation::default()
        } else {
            ContextMutation {
                messages: Some(current_messages),
            }
        }
    }

    async fn on_before_agent_start(
        &self,
        ctx: &BeforeAgentStartCtx,
    ) -> BeforeAgentStartMutation {
        let mut accumulated = BeforeAgentStartMutation::default();
        let mut current_ctx = ctx.clone();

        for handle in &self.handles {
            tracing::debug!(extension = %handle.name, hook = "on_before_agent_start", "dispatching hook");
            let mutation = handle.on_before_agent_start(current_ctx.clone()).await;
            if let Some(ref sp) = mutation.system_prompt {
                current_ctx.system_prompt = Some(sp.clone());
                accumulated.system_prompt = Some(sp.clone());
            }
            if let Some(ref msgs) = mutation.messages {
                current_ctx.messages = msgs.clone();
                accumulated.messages = Some(msgs.clone());
            }
        }
        accumulated
    }

    async fn on_before_provider_request(
        &self,
        ctx: &ProviderRequestCtx,
    ) -> ProviderRequestMutation {
        let mut accumulated = ProviderRequestMutation::default();
        let mut current_ctx = ctx.clone();

        for handle in &self.handles {
            tracing::debug!(extension = %handle.name, hook = "on_before_provider_request", "dispatching hook");
            let mutation = handle.on_before_provider_request(current_ctx.clone()).await;
            if let Some(sp) = mutation.system_prompt {
                if let Some(sp_str) = &sp {
                    current_ctx.system_prompt = Some(sp_str.clone());
                }
                accumulated.system_prompt = Some(sp);
            }
            if let Some(ref msgs) = mutation.messages {
                current_ctx.messages = msgs.clone();
                accumulated.messages = Some(msgs.clone());
            }
            if let Some(ref tools) = mutation.tools {
                current_ctx.tools = tools.clone();
                accumulated.tools = Some(tools.clone());
            }
            if let Some(ref options) = mutation.options {
                current_ctx.options = options.clone();
                accumulated.options = Some(options.clone());
            }
        }
        accumulated
    }

    async fn on_after_provider_response(
        &self,
        ctx: &ProviderResponseCtx,
    ) -> ProviderResponseMutation {
        let mut accumulated = ProviderResponseMutation::default();
        let mut current_ctx = ctx.clone();

        for handle in &self.handles {
            tracing::debug!(extension = %handle.name, hook = "on_after_provider_response", "dispatching hook");
            let mutation = handle.on_after_provider_response(current_ctx.clone()).await;
            if let Some(ref content) = mutation.content {
                current_ctx.content = content.clone();
                accumulated.content = Some(content.clone());
            }
            if let Some(ref stop_reason) = mutation.stop_reason {
                current_ctx.stop_reason = stop_reason.clone();
                accumulated.stop_reason = Some(stop_reason.clone());
            }
        }
        accumulated
    }

    // ═══ Observational hooks — fire-and-forget ═══

    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        self.event_bus.emit(ObsEvent::TurnEnd(ctx.clone()));
    }

    async fn on_agent_end(&self, ctx: &AgentEndCtx) {
        self.event_bus.emit(ObsEvent::AgentEnd(ctx.clone()));
    }

    async fn on_session_start(&self, ctx: &SessionCtx) {
        self.event_bus.emit(ObsEvent::SessionStart(ctx.clone()));
    }

    async fn on_tool_execution_start(&self, ctx: &ToolExecutionStartCtx) {
        self.event_bus.emit(ObsEvent::ToolExecutionStart(ctx.clone()));
    }

    async fn on_tool_execution_end(&self, ctx: &ToolExecutionEndCtx) {
        self.event_bus.emit(ObsEvent::ToolExecutionEnd(ctx.clone()));
    }

    async fn on_compact_end(&self, ctx: &CompactEndCtx) {
        self.event_bus.emit(ObsEvent::CompactEnd(ctx.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use agent_core::HookDispatcher;
    use crate::host::extension::Extension;
    use crate::host::extension_actor::ExtensionActor;

    struct ContinueExt(String);
    #[async_trait]
    impl Extension for ContinueExt {
        fn name(&self) -> &str { &self.0 }
    }

    struct BlockExt(String);
    #[async_trait]
    impl Extension for BlockExt {
        fn name(&self) -> &str { &self.0 }
        async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
            (HookDecision::Block { reason: "no".to_string() }, ToolCallMutation::default())
        }
    }

    #[tokio::test]
    async fn test_hook_router_first_block_wins() {
        let bus = Arc::new(EventBus::<ObsEvent>::new(16));

        let ext1 = Arc::new(ContinueExt("ext1".to_string()));
        let ext2 = Arc::new(BlockExt("ext2".to_string()));
        let ext3 = Arc::new(ContinueExt("ext3".to_string()));

        let (h1, _jh1) = ExtensionActor::spawn(ext1, bus.clone(), 8);
        let (h2, _jh2) = ExtensionActor::spawn(ext2, bus.clone(), 8);
        let (h3, _jh3) = ExtensionActor::spawn(ext3, bus.clone(), 8);

        let router = HookRouter::new(vec![h1, h2, h3], bus);

        let ctx = ToolCallCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            tool_name: "t".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({}),
        };

        let (decision, _mutation) = router.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Block { .. }));
    }

    #[tokio::test]
    async fn test_hook_router_all_continue() {
        let bus = Arc::new(EventBus::<ObsEvent>::new(16));

        let ext1 = Arc::new(ContinueExt("ext1".to_string()));
        let ext2 = Arc::new(ContinueExt("ext2".to_string()));

        let (h1, _jh1) = ExtensionActor::spawn(ext1, bus.clone(), 8);
        let (h2, _jh2) = ExtensionActor::spawn(ext2, bus.clone(), 8);

        let router = HookRouter::new(vec![h1, h2], bus);

        let ctx = ToolCallCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            tool_name: "t".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({}),
        };

        let (decision, _mutation) = router.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Continue));
    }
}
