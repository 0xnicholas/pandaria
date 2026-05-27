//! `MemoryHookDispatcher` — sends formatted turn content to external memory
//! systems and retrieves memories for context injection.
//!
//! Pandaria does NOT do fact extraction. The external system (Emerald) handles
//! extraction, chunking, embedding, and relationship inference. Pandaria's job
//! is to format raw data and pass it along.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::hook::context::{CompactEndCtx, ContextCtx, TurnEndCtx};
use crate::hook::dispatcher::HookDispatcher;
use crate::hook::mutations::ContextMutation;
use crate::types::AgentMessage;

use super::extractor::build_query_string;
use super::formatter::{build_turn_metadata, extract_tool_summaries, format_turn_content};
use super::store::MemoryStore;
use super::types::MemoryContext;

/// `HookDispatcher` implementation that sends formatted turn content
/// to an external `MemoryStore` and retrieves memories for context injection.
pub struct MemoryHookDispatcher {
    store: Arc<dyn MemoryStore>,
    model: String,
    session_started_at: SystemTime,
}

impl MemoryHookDispatcher {
    pub fn new(
        store: Arc<dyn MemoryStore>,
        model: String,
        session_started_at: SystemTime,
    ) -> Self {
        Self {
            store,
            model,
            session_started_at,
        }
    }

    fn make_ctx(&self, tenant_id: &str, session_id: &str) -> MemoryContext {
        MemoryContext {
            tenant_id: tenant_id.to_string(),
            session_id: session_id.to_string(),
            user_id: None,
            model: self.model.clone(),
            session_started_at: self.session_started_at,
        }
    }
}

#[async_trait]
impl HookDispatcher for MemoryHookDispatcher {
    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        let content = format_turn_content(ctx.turn_index, &ctx.messages);
        let tool_summaries = extract_tool_summaries(&ctx.messages);
        let metadata = build_turn_metadata(
            &ctx.tenant_id,
            &ctx.session_id,
            ctx.turn_index,
            &self.model,
            &ctx.usage,
            "stop", // TurnEndCtx doesn't carry stop_reason
            &tool_summaries,
            SystemTime::now(),
        );

        let mem_ctx = self.make_ctx(&ctx.tenant_id, &ctx.session_id);
        let store = self.store.clone();
        let turn_index = ctx.turn_index;

        tokio::spawn(async move {
            match tokio::time::timeout(
                Duration::from_secs(5),
                store.remember(&mem_ctx, &content, &metadata),
            )
            .await
            {
                Ok(Ok(())) => debug!(turn_index, "memory: remembered turn"),
                Ok(Err(e)) => warn!(turn_index, error = %e, "memory: remember failed"),
                Err(_) => warn!(turn_index, "memory: remember timed out"),
            }
        });
    }

    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        let query = build_query_string(&ctx.messages);
        if query.is_empty() {
            return ContextMutation::default();
        }

        let mem_ctx = self.make_ctx(&ctx.tenant_id, &ctx.session_id);
        let facts = match tokio::time::timeout(
            Duration::from_secs(3),
            self.store.recall(&mem_ctx, &query),
        )
        .await
        {
            Ok(Ok(facts)) if !facts.is_empty() => facts,
            Ok(Err(e)) => {
                warn!(error = %e, "memory: recall failed");
                return ContextMutation::default();
            }
            Err(_) => {
                warn!("memory: recall timed out");
                return ContextMutation::default();
            }
            _ => return ContextMutation::default(),
        };

        debug!(fact_count = facts.len(), "memory: injecting recalled facts");

        let memory_text = facts.join("\n---\n");
        let memory_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: format!("[Memory]\n{}", memory_text),
                text_signature: None,
            }],
            timestamp: SystemTime::now(),
        });

        let mut messages = ctx.messages.clone();
        messages.insert(0, memory_msg);
        ContextMutation {
            messages: Some(messages),
        }
    }

    async fn on_compact_end(&self, ctx: &CompactEndCtx) {
        let summary = match &ctx.result {
            Some(r) => format!("[Session Compaction Summary]\n{}", r.summary),
            None => return,
        };

        let metadata = serde_json::json!({
            "category": "compaction",
            "importance": 8,
            "session_id": ctx.session_id,
            "token_savings": ctx.token_savings,
        });

        let mem_ctx = self.make_ctx(&ctx.tenant_id, &ctx.session_id);
        let store = self.store.clone();

        tokio::spawn(async move {
            match tokio::time::timeout(
                Duration::from_secs(5),
                store.remember(&mem_ctx, &summary, &metadata),
            )
            .await
            {
                Ok(Ok(())) => debug!("memory: compaction summary remembered"),
                Ok(Err(e)) => warn!(error = %e, "memory: compaction summary failed"),
                Err(_) => warn!("memory: compaction summary timed out"),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::in_memory::InMemoryStore;
    use ai_provider::{AssistantMessage, Content, StopReason, Usage, UserMessage};

    fn make_ctx() -> TurnEndCtx {
        TurnEndCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            turn_index: 1,
            messages: vec![
                AgentMessage::User(UserMessage {
                    content: vec![Content::Text {
                        text: "hello".to_string(),
                        text_signature: None,
                    }],
                    timestamp: SystemTime::now(),
                }),
                AgentMessage::Assistant(AssistantMessage {
                    content: vec![Content::Text {
                        text: "Hi! How can I help?".to_string(),
                        text_signature: None,
                    }],
                    provider: "test".into(),
                    model: "test".into(),
                    api: ai_provider::Api {
                        provider: "test".into(),
                        model: "test".into(),
                    },
                    usage: Usage {
                        input_tokens: 0, output_tokens: 0, total_tokens: 0,
                        cache_creation_input_tokens: None, cache_read_input_tokens: None,
                    },
                    stop_reason: StopReason::Stop,
                    response_id: None,
                    error_message: None,
                    timestamp: SystemTime::now(),
                }),
            ],
            usage: Usage {
                input_tokens: 0, output_tokens: 0, total_tokens: 0,
                cache_creation_input_tokens: None, cache_read_input_tokens: None,
            },
        }
    }

    #[tokio::test]
    async fn test_on_turn_end_remembers_content() {
        let store = Arc::new(InMemoryStore::new());
        let dispatcher = MemoryHookDispatcher::new(
            store.clone(),
            "gpt-4".into(),
            SystemTime::now(),
        );

        let ctx = make_ctx();
        dispatcher.on_turn_end(&ctx).await;

        // Give fire-and-forget a moment
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mem_ctx = dispatcher.make_ctx("t1", "s1");
        let results = store.recall(&mem_ctx, "hello").await.unwrap();
        assert!(!results.is_empty());
        assert!(results[0].contains("hello"));
    }

    #[tokio::test]
    async fn test_on_context_recalls_memories() {
        let store = Arc::new(InMemoryStore::new());
        let dispatcher = MemoryHookDispatcher::new(
            store.clone(),
            "gpt-4".into(),
            SystemTime::now(),
        );

        // Seed a memory
        let mem_ctx = dispatcher.make_ctx("t1", "s1");
        store
            .remember(&mem_ctx, "Rust is fast", &serde_json::json!({}))
            .await
            .unwrap();

        let ctx = ContextCtx {
            tenant_id: "t1".into(),
            session_id: "s1".into(),
            messages: vec![AgentMessage::User(UserMessage {
                content: vec![Content::Text {
                    text: "Rust".into(),
                    text_signature: None,
                }],
                timestamp: SystemTime::now(),
            })],
        };
        let mutation = dispatcher.on_context(&ctx).await;
        let msgs = mutation.messages.expect("should have messages");
        assert_eq!(msgs.len(), 2); // memory msg + original
        assert!(matches!(&msgs[0], AgentMessage::User(_)));
    }

    #[tokio::test]
    async fn test_on_context_empty_query_returns_default() {
        let store = Arc::new(InMemoryStore::new());
        let dispatcher = MemoryHookDispatcher::new(
            store.clone(),
            "gpt-4".into(),
            SystemTime::now(),
        );

        let ctx = ContextCtx {
            tenant_id: "t1".into(),
            session_id: "s1".into(),
            messages: vec![],
        };
        let mutation = dispatcher.on_context(&ctx).await;
        assert!(mutation.messages.is_none());
    }
}
