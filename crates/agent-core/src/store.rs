use async_trait::async_trait;

use crate::error::AgentError;
use crate::types::AgentMessage;

/// Persistence boundary for session message history.
///
/// Implementations in the `persistence` crate provide Redis, PostgreSQL,
/// or in-memory storage. Defined here as a trait so `agent-core` remains
/// storage-agnostic.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Persist the full message history for a session.
    async fn save_session(
        &self,
        tenant_id: &str,
        session_id: &str,
        messages: &[AgentMessage],
    ) -> Result<(), AgentError>;

    /// Load the full message history for a session.
    async fn load_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<Vec<AgentMessage>, AgentError>;
}
