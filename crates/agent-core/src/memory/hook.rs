use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::hook::context::{CompactEndCtx, ContextCtx, TurnEndCtx};
use crate::hook::dispatcher::HookDispatcher;
use crate::hook::mutations::ContextMutation;
use crate::types::AgentMessage;

use super::extractor::{build_query, extract_facts, format_facts};
use super::store::MemoryStore;
use super::types::MemoryContext;

/// `HookDispatcher` implementation that automatically writes and retrieves
/// memories from an external `MemoryStore`.
///
/// - `on_turn_end`   → extracts facts and fire-and-forget `remember`
/// - `on_context`    → builds query and injects retrieved facts
/// - `on_compact_end`→ persists compaction summary as a long-term memory
pub struct MemoryHookDispatcher {
    store: Arc<dyn MemoryStore>,
}

impl MemoryHookDispatcher {
    pub fn new(store: Arc<dyn MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl HookDispatcher for MemoryHookDispatcher {
    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        let facts = extract_facts(&ctx.messages);
        if facts.is_empty() {
            return;
        }

        let mem_ctx = MemoryContext {
            tenant_id: ctx.tenant_id.clone(),
            session_id: ctx.session_id.clone(),
            user_id: None,
        };

        debug!(
            tenant_id = %ctx.tenant_id,
            session_id = %ctx.session_id,
            fact_count = facts.len(),
            "memory: remembering turn facts"
        );

        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            self.store.remember(&mem_ctx, &facts),
        ).await;
    }

    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        let query = build_query(&ctx.messages);
        if query.text.is_empty() {
            return ContextMutation::default();
        }

        let mem_ctx = MemoryContext {
            tenant_id: ctx.tenant_id.clone(),
            session_id: ctx.session_id.clone(),
            user_id: None,
        };

        let facts = match tokio::time::timeout(
            Duration::from_secs(3),
            self.store.recall(&mem_ctx, &query),
        ).await
        {
            Ok(Ok(facts)) if !facts.is_empty() => facts,
            Ok(Err(e)) => {
                warn!(
                    tenant_id = %ctx.tenant_id,
                    session_id = %ctx.session_id,
                    error = %e,
                    "memory: recall failed"
                );
                return ContextMutation::default();
            }
            _ => return ContextMutation::default(),
        };

        debug!(
            tenant_id = %ctx.tenant_id,
            session_id = %ctx.session_id,
            fact_count = facts.len(),
            "memory: injecting recalled facts into context"
        );

        let memory_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: format!("[Memory]\n{}", format_facts(&facts)),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let mut messages = ctx.messages.clone();
        messages.insert(0, memory_msg);
        ContextMutation { messages: Some(messages) }
    }

    async fn on_compact_end(&self, ctx: &CompactEndCtx) {
        let summary = match &ctx.result {
            Some(r) => r.summary.clone(),
            None => return,
        };

        let fact = super::types::MemoryFact {
            id: None,
            content: format!("[Session Compaction Summary]\n{}", summary),
            category: Some("compaction".to_string()),
            importance: Some(8),
            metadata: serde_json::json!({
                "session_id": ctx.session_id,
                "token_savings": ctx.token_savings,
            }),
        };

        let mem_ctx = MemoryContext {
            tenant_id: ctx.tenant_id.clone(),
            session_id: ctx.session_id.clone(),
            user_id: None,
        };

        debug!(
            tenant_id = %ctx.tenant_id,
            session_id = %ctx.session_id,
            "memory: remembering compaction summary"
        );

        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            self.store.remember(&mem_ctx, &[fact]),
        ).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::in_memory::InMemoryStore;
    use crate::memory::types::MemoryQuery;
    use ai_provider::Usage;

    #[tokio::test]
    async fn test_memory_hook_dispatcher_on_turn_end() {
        let store = Arc::new(InMemoryStore::new());
        let dispatcher = MemoryHookDispatcher::new(store.clone());

        let ctx = TurnEndCtx::new("t1", "s1", 1, ai_provider::Usage { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: None, cache_read_input_tokens: None, total_tokens: 0 });
        dispatcher.on_turn_end(&ctx).await;

        // No messages → no facts → store should be empty
        let facts = store.recall(
            &MemoryContext { tenant_id: "t1".to_string(), session_id: "s1".to_string(), user_id: None },
            &MemoryQuery { text: "".to_string(), limit: 10, session_only: false },
        ).await.unwrap();
        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn test_memory_hook_dispatcher_on_context_empty_query() {
        let store = Arc::new(InMemoryStore::new());
        let dispatcher = MemoryHookDispatcher::new(store.clone());

        let ctx = ContextCtx::new("t1", "s1");
        let mutation = dispatcher.on_context(&ctx).await;
        assert!(mutation.messages.is_none());
    }
}
