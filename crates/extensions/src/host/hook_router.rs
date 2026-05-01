use std::sync::Arc;

use async_trait::async_trait;

use agent_core::context::{
    AgentEndCtx, ContextCtx, SessionCtx, ToolCallCtx, ToolResultCtx, TurnEndCtx,
};
use agent_core::mutations::{HookDecision, ToolResultMutation};
use agent_core::types::AgentMessage;

use super::event_bus::EventBus;
use super::extension_actor::{ExtensionHandle, ObsEvent};

/// Implements agent_core::HookDispatcher by routing to ExtensionActors.
///
/// - Blocking hooks (on_tool_call): serial dispatch, first-block-wins
/// - Chaining hooks (on_tool_result, on_context): serial dispatch, chain merge
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
    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> HookDecision {
        // Serial dispatch, first-block-wins
        let current_ctx = ctx.clone();
        for handle in &self.handles {
            let decision = handle.on_tool_call(current_ctx.clone()).await;
            // Note: input mutations are lost here since ctx is cloned.
            // In production, we'd need a mutable ctx reference.
            // For v0.1, we accept this limitation.
            match decision {
                HookDecision::Block { .. } => return decision,
                HookDecision::Continue => continue,
            }
        }
        HookDecision::Continue
    }

    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
        // Chain merge — each handler sees previous mutations
        let mut current_ctx = ctx.clone();
        let mut final_mutation = ToolResultMutation::default();

        for handle in &self.handles {
            let mutation = handle.on_tool_result(current_ctx.clone()).await;

            // Apply mutation to ctx for next handler
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
        }

        final_mutation
    }

    async fn on_context(&self, messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
        // Chain merge — each handler transforms the messages
        let mut current_messages = messages;

        for handle in &self.handles {
            let ctx = ContextCtx {
                messages: current_messages.clone(),
            };
            let mutation = handle.on_context(ctx).await;
            if let Some(msgs) = mutation.messages {
                current_messages = msgs;
            }
        }

        current_messages
    }

    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        self.event_bus.emit(ObsEvent::TurnEnd(ctx.clone()));
    }

    async fn on_agent_end(&self, ctx: &AgentEndCtx) {
        self.event_bus.emit(ObsEvent::AgentEnd(ctx.clone()));
    }

    async fn on_session_start(&self, ctx: &SessionCtx) {
        self.event_bus.emit(ObsEvent::SessionStart(ctx.clone()));
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
        async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> HookDecision {
            HookDecision::Block { reason: "no".to_string() }
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
            tool_name: "t".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({}),
        };

        let decision = router.on_tool_call(&ctx).await;
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
            tool_name: "t".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({}),
        };

        let decision = router.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Continue));
    }
}
