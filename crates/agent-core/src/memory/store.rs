use async_trait::async_trait;

use super::types::MemoryContext;

/// Protocol boundary for external memory systems.
///
/// Pandaria does not implement storage / retrieval / embedding itself.
/// Any external system (Emerald, SuperMemory, Mem0, in-house service, etc.)
/// can be plugged in by implementing this trait.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Send formatted conversation content to the external memory system.
    ///
    /// `content` is a Markdown-formatted turn transcript. The external system
    /// handles extraction, chunking, embedding, and relationship inference.
    /// `metadata` carries structured context (turn_index, model, token_usage, etc.).
    ///
    /// Failures should be silently discarded by the caller (MemoryHookDispatcher)
    /// so they never block the agent loop.
    async fn remember(
        &self,
        ctx: &MemoryContext,
        content: &str,
        metadata: &serde_json::Value,
    ) -> Result<(), MemoryError>;

    /// Retrieve relevant memories for a query. Returns plain text strings.
    async fn recall(
        &self,
        ctx: &MemoryContext,
        query: &str,
    ) -> Result<Vec<String>, MemoryError>;

    /// Optional: delete all memories associated with a session.
    /// Default no-op for stores that do not support per-session eviction.
    async fn forget_session(&self, _ctx: &MemoryContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("memory store error: {0}")]
    StoreError(String),
}
