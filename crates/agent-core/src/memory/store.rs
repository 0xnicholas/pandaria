use async_trait::async_trait;

use super::types::{MemoryContext, MemoryFact, MemoryQuery};

/// Protocol boundary for external memory systems.
///
/// Pandaria does not implement storage / retrieval / embedding itself.
/// Any external system (SuperMemory, Mem0, in-house service, etc.) can
/// be plugged in by implementing this trait and passing the instance
/// via `RuntimeConfig.memory_store`.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Persist facts.  Failures should be silently discarded by the caller
    /// (`MemoryHookDispatcher`) so they never block the agent loop.
    async fn remember(&self, ctx: &MemoryContext, facts: &[MemoryFact]) -> Result<(), MemoryError>;

    /// Retrieve relevant facts.  Returned facts are injected directly into
    /// the LLM context.
    async fn recall(&self, ctx: &MemoryContext, query: &MemoryQuery) -> Result<Vec<MemoryFact>, MemoryError>;

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
