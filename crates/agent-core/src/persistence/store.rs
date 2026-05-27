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
    async fn delete_session(&self, tenant_id: &str, session_id: &str) -> Result<(), AgentError>;

    /// List all session IDs for a given tenant.
    async fn list_sessions(&self, tenant_id: &str) -> Result<Vec<String>, AgentError>;

    /// Append new entries to an existing session without a full load→merge→save.
    ///
    /// The name reflects the caller's intent ("I have new entries to append"),
    /// not a guarantee of physical append at the storage layer.
    ///
    /// Default implementation: load → merge → full save.
    /// Storage adapters may override with more efficient strategies
    /// (e.g., `jsonb_insert` for PostgreSQL).
    async fn append_entries(
        &self,
        tenant_id: &str,
        session_id: &str,
        new_entries: &[SessionEntry],
    ) -> Result<(), AgentError> {
        let mut all = self.load_session(tenant_id, session_id).await?;
        all.extend_from_slice(new_entries);
        self.save_session(tenant_id, session_id, &all).await
    }
}
