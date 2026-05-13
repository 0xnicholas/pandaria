use async_trait::async_trait;

use crate::error::AgentError;
use crate::persistence::entry::SessionEntry;

/// Persistence boundary for session history (messages + compaction metadata).
///
/// Implementations in the `storage` crate provide Redis, PostgreSQL,
/// or in-memory storage. Defined here as a trait so `agent-core` remains
/// storage-agnostic.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Persist the full session history for a session.
    async fn save_session(
        &self,
        tenant_id: &str,
        session_id: &str,
        entries: &[SessionEntry],
    ) -> Result<(), AgentError>;

    /// Load the full session history for a session.
    async fn load_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<Vec<SessionEntry>, AgentError>;

    /// Delete a session and all its data.
    async fn delete_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<(), AgentError>;

    /// List all session IDs for a given tenant.
    async fn list_sessions(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<String>, AgentError>;
}
