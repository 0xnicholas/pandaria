use std::sync::Arc;

use async_trait::async_trait;

use super::context::*;
use super::dispatcher::HookDispatcher;
use super::mutations::*;

/// Combines multiple `HookDispatcher` instances into a single dispatcher.
///
/// - **Blocking hooks** (`on_tool_call`, `on_before_compact`): first-block-wins.
///   Dispatches are evaluated in order; the first one that returns a `Block`
///   decision stops the chain.
/// - **Chaining hooks** (`on_context`, `on_before_agent_start`,
///   `on_before_provider_request`, `on_after_provider_response`): pipeline mode.
///   Each child dispatcher sees the output of the previous one.
/// - **Observational hooks** (`on_turn_end`, `on_agent_end`, etc.): sequential
///   fire-and-forget.
pub struct CombinedDispatcher {
    chain: Vec<Arc<dyn HookDispatcher>>,
}

impl CombinedDispatcher {
    /// Create a new `CombinedDispatcher` from a chain of dispatchers.
    pub fn new(chain: Vec<Arc<dyn HookDispatcher>>) -> Self {
        Self { chain }
    }
}

#[async_trait]
impl HookDispatcher for CombinedDispatcher {
    // ------------------------------------------------------------------
    // Blocking hooks — first-block-wins
    // ------------------------------------------------------------------

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        for d in &self.chain {
            let (decision, mutation) = d.on_tool_call(ctx).await;
            if matches!(decision, HookDecision::Block { .. }) {
                return (decision, mutation);
            }
        }
        (HookDecision::Continue, ToolCallMutation::default())
    }

    async fn on_before_compact(&self, ctx: &CompactCtx) -> CompactDecision {
        for d in &self.chain {
            let decision = d.on_before_compact(ctx).await;
            if matches!(
                decision,
                CompactDecision::Block { .. } | CompactDecision::Replace { .. }
            ) {
                return decision;
            }
        }
        CompactDecision::Continue
    }

    // ------------------------------------------------------------------
    // Chaining hooks — pipeline mode
    // ------------------------------------------------------------------

    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
        let mut mutation = ToolResultMutation::default();
        for d in &self.chain {
            let next = d.on_tool_result(ctx).await;
            if next.content.is_some() {
                mutation.content = next.content;
            }
            if next.details.is_some() {
                mutation.details = next.details;
            }
            if next.is_error.is_some() {
                mutation.is_error = next.is_error;
            }
            if next.terminate.is_some() {
                mutation.terminate = next.terminate;
            }
        }
        mutation
    }

    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        let mut messages = ctx.messages.clone();
        for d in &self.chain {
            let mutation = d
                .on_context(&ContextCtx {
                    tenant_id: ctx.tenant_id.clone(),
                    session_id: ctx.session_id.clone(),
                    messages: messages.clone(),
                })
                .await;
            if let Some(msgs) = mutation.messages {
                messages = msgs;
            }
        }
        ContextMutation {
            messages: Some(messages),
        }
    }

    async fn on_before_agent_start(
        &self,
        ctx: &BeforeAgentStartCtx,
    ) -> BeforeAgentStartMutation {
        let mut mutation = BeforeAgentStartMutation::default();
        let mut prompt_builder = ctx.prompt_builder.clone();
        let mut messages = ctx.messages.clone();
        for d in &self.chain {
            let next = d
                .on_before_agent_start(&BeforeAgentStartCtx {
                    tenant_id: ctx.tenant_id.clone(),
                    session_id: ctx.session_id.clone(),
                    system_prompt: ctx.system_prompt.clone(),
                    prompt_builder: prompt_builder.clone(),
                    messages: messages.clone(),
                    tools: ctx.tools.clone(),
                    model: ctx.model.clone(),
                })
                .await;
            if let Some(pb) = next.system_prompt {
                prompt_builder = pb;
            }
            if next.prompt_mutation.is_some() {
                mutation.prompt_mutation = next.prompt_mutation;
            }
            if let Some(msgs) = next.messages {
                messages = msgs;
            }
        }
        BeforeAgentStartMutation {
            system_prompt: Some(prompt_builder),
            prompt_mutation: mutation.prompt_mutation,
            messages: Some(messages),
        }
    }

    async fn on_before_provider_request(
        &self,
        ctx: &ProviderRequestCtx,
    ) -> ProviderRequestMutation {
        let mut mutation = ProviderRequestMutation::default();
        let mut prompt_builder = ctx.prompt_builder.clone();
        let mut messages = ctx.messages.clone();
        let mut tools: Option<Vec<ai_provider::ToolDef>> = ctx.tools.clone();
        let mut options = ctx.options.clone();
        for d in &self.chain {
            let next = d
                .on_before_provider_request(&ProviderRequestCtx {
                    tenant_id: ctx.tenant_id.clone(),
                    session_id: ctx.session_id.clone(),
                    model: ctx.model.clone(),
                    system_prompt: ctx.system_prompt.clone(),
                    prompt_builder: prompt_builder.clone(),
                    messages: messages.clone(),
                    turn_index: ctx.turn_index,
                    tools: tools.clone(),
                    options: options.clone(),
                })
                .await;
            if let Some(pb) = next.system_prompt {
                prompt_builder = pb;
            }
            if next.prompt_mutation.is_some() {
                mutation.prompt_mutation = next.prompt_mutation;
            }
            if let Some(msgs) = next.messages {
                messages = msgs;
            }
            if let Some(t) = next.tools {
                tools = t;
            }
            if let Some(o) = next.options {
                options = o;
            }
        }
        ProviderRequestMutation {
            system_prompt: Some(prompt_builder),
            prompt_mutation: mutation.prompt_mutation,
            messages: Some(messages),
            tools: Some(tools),
            options: Some(options),
        }
    }

    async fn on_after_provider_response(
        &self,
        ctx: &ProviderResponseCtx,
    ) -> ProviderResponseMutation {
        let mut mutation = ProviderResponseMutation::default();
        for d in &self.chain {
            let next = d
                .on_after_provider_response(&ProviderResponseCtx {
                    tenant_id: ctx.tenant_id.clone(),
                    session_id: ctx.session_id.clone(),
                    model: ctx.model.clone(),
                    content: mutation
                        .content
                        .clone()
                        .unwrap_or_else(|| ctx.content.clone()),
                    turn_index: ctx.turn_index,
                    attempt: ctx.attempt,
                    messages_before: ctx.messages_before.clone(),
                    stop_reason: mutation.stop_reason.clone().unwrap_or_else(|| ctx.stop_reason.clone()),
                })
                .await;
            if next.content.is_some() {
                mutation.content = next.content;
            }
            if next.stop_reason.is_some() {
                mutation.stop_reason = next.stop_reason;
            }
        }
        mutation
    }

    // ------------------------------------------------------------------
    // Observational hooks — fire-and-forget, sequential
    // ------------------------------------------------------------------

    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        for d in &self.chain {
            d.on_turn_end(ctx).await;
        }
    }

    async fn on_agent_end(&self, ctx: &AgentEndCtx) {
        for d in &self.chain {
            d.on_agent_end(ctx).await;
        }
    }

    async fn on_session_start(&self, ctx: &SessionCtx) {
        for d in &self.chain {
            d.on_session_start(ctx).await;
        }
    }

    async fn on_tool_execution_start(&self, ctx: &ToolExecutionStartCtx) {
        for d in &self.chain {
            d.on_tool_execution_start(ctx).await;
        }
    }

    async fn on_tool_execution_end(&self, ctx: &ToolExecutionEndCtx) {
        for d in &self.chain {
            d.on_tool_execution_end(ctx).await;
        }
    }

    async fn on_compact_end(&self, ctx: &CompactEndCtx) {
        for d in &self.chain {
            d.on_compact_end(ctx).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::context::*;
    use crate::hook::dispatcher::HookDispatcher;
    use crate::hook::mutations::*;
    use crate::types::AgentMessage;

    struct BlockToolDispatcher;

    #[async_trait]
    impl HookDispatcher for BlockToolDispatcher {
        async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
            (
                HookDecision::Block {
                    reason: "blocked".to_string(),
                },
                ToolCallMutation::default(),
            )
        }
    }

    struct PassThroughDispatcher;

    #[async_trait]
    impl HookDispatcher for PassThroughDispatcher {
        // all defaults are pass-through
    }

    struct AppendMessageDispatcher {
        prefix: String,
    }

    #[async_trait]
    impl HookDispatcher for AppendMessageDispatcher {
        async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
            let mut messages = ctx.messages.clone();
            messages.push(AgentMessage::User(ai_provider::UserMessage {
                content: vec![ai_provider::Content::Text {
                    text: self.prefix.clone(),
                    text_signature: None,
                }],
                timestamp: std::time::SystemTime::now(),
            }));
            ContextMutation {
                messages: Some(messages),
            }
        }
    }

    #[tokio::test]
    async fn test_combined_blocking_first_block_wins() {
        let combined = CombinedDispatcher::new(vec![
            Arc::new(PassThroughDispatcher),
            Arc::new(BlockToolDispatcher),
            Arc::new(PassThroughDispatcher),
        ]);

        let ctx = ToolCallCtx::new("t1", "s1", "tool", "tc1");
        let (decision, _) = combined.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Block { .. }));
    }

    #[tokio::test]
    async fn test_combined_blocking_continue_when_none_block() {
        let combined = CombinedDispatcher::new(vec![
            Arc::new(PassThroughDispatcher),
            Arc::new(PassThroughDispatcher),
        ]);

        let ctx = ToolCallCtx::new("t1", "s1", "tool", "tc1");
        let (decision, _) = combined.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Continue));
    }

    #[tokio::test]
    async fn test_combined_context_pipeline() {
        let combined = CombinedDispatcher::new(vec![
            Arc::new(AppendMessageDispatcher {
                prefix: "first".to_string(),
            }),
            Arc::new(AppendMessageDispatcher {
                prefix: "second".to_string(),
            }),
        ]);

        let ctx = ContextCtx::new("t1", "s1");
        let mutation = combined.on_context(&ctx).await;
        let messages = mutation.messages.expect("messages should be set");
        assert_eq!(messages.len(), 2);
        // Both messages appended in order
    }

    #[tokio::test]
    async fn test_combined_empty_chain_returns_defaults() {
        let combined = CombinedDispatcher::new(vec![]);

        let ctx = ToolCallCtx::new("t1", "s1", "tool", "tc1");
        let (decision, _) = combined.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Continue));

        let ctx = ContextCtx::new("t1", "s1");
        let mutation = combined.on_context(&ctx).await;
        assert!(mutation.messages.is_some());
        assert!(mutation.messages.unwrap().is_empty());
    }
}
