//! SessionHistory — owns message entries + persistence + restore + flush.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;

use crate::error::AgentError;
use crate::harness::compaction::estimate_context_tokens;
use crate::persistence::entry::SessionEntry;
use crate::persistence::store::SessionStore;
use crate::types::AgentMessage;

pub struct SessionHistory {
    tenant_id: String,
    session_id: String,
    entries: Vec<SessionEntry>,
    store: Option<Arc<dyn SessionStore>>,
    needs_restore: bool,
    last_saved_entry_count: usize,
    last_save: Option<JoinHandle<()>>,
}

impl SessionHistory {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        store: Option<Arc<dyn SessionStore>>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            entries: Vec::new(),
            store,
            needs_restore: true,
            last_saved_entry_count: 0,
            last_save: None,
        }
    }

    // ── Message operations ──

    pub fn push(&mut self, msg: AgentMessage) {
        self.entries.push(SessionEntry::Message {
            id: uuid::Uuid::new_v4(),
            message: msg,
        });
    }

    pub fn append_compaction(&mut self, entry: SessionEntry) {
        self.entries.push(entry);
    }

    /// Remove entries strictly before the boundary (the boundary entry itself is kept).
    /// No-op if the boundary ID is not found.
    pub fn truncate_before(&mut self, boundary: uuid::Uuid) {
        if let Some(idx) = self.entries.iter().position(|e| e.id() == boundary) {
            self.entries.drain(..idx);
        }
    }

    // ── Reads ──

    pub fn messages(&self) -> Vec<AgentMessage> {
        self.entries.iter().filter_map(|e| match e {
            SessionEntry::Message { message, .. } => Some(message.clone()),
            SessionEntry::Compaction { .. } => None,
        }).collect()
    }

    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn last_compaction_timestamp(&self) -> Option<SystemTime> {
        self.entries.iter().rev().find_map(|e| match e {
            SessionEntry::Compaction { timestamp, .. } => Some(*timestamp),
            SessionEntry::Message { .. } => None,
        })
    }

    // ── Persistence ──

    /// First call: load from store. Subsequent calls: no-op.
    /// On load failure, keeps `needs_restore = false` (avoid retry loop) but logs.
    pub async fn auto_restore(&mut self) -> Result<(), AgentError> {
        if !self.needs_restore {
            return Ok(());
        }
        self.needs_restore = false;
        if let Some(ref store) = self.store {
            match store.load_session(&self.tenant_id, &self.session_id).await {
                Ok(entries) if !entries.is_empty() => {
                    let count = entries.len();
                    self.entries = entries;
                    self.last_saved_entry_count = count;
                    tracing::info!(
                        tenant_id = %self.tenant_id,
                        session_id = %self.session_id,
                        restored_count = count,
                        "auto-restored session history",
                    );
                }
                Ok(_) => {} // empty store, fresh session
                Err(e) => {
                    tracing::warn!(
                        tenant_id = %self.tenant_id,
                        session_id = %self.session_id,
                        error = %e,
                        "auto-restore failed, starting with empty session",
                    );
                }
            }
        }
        Ok(())
    }

    /// Save only newly added entries since the last save boundary.
    /// Awaits the previous in-flight save (with 5s timeout) to preserve ordering,
    /// then spawns a new fire-and-forget save task.
    #[allow(dead_code)] // Called by SessionActor after delegation arrives in Task 5/6
    pub async fn persist_incremental(&mut self) {
        let Some(ref store) = self.store else { return; };
        if let Some(handle) = self.last_save.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }
        let new_entries = &self.entries[self.last_saved_entry_count..];
        if new_entries.is_empty() {
            return;
        }
        let entries_to_save = new_entries.to_vec();
        self.last_saved_entry_count = self.entries.len();
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();
        let store = store.clone();
        self.last_save = Some(tokio::spawn(async move {
            if let Err(e) = store.append_entries(&tenant_id, &session_id, &entries_to_save).await {
                tracing::warn!(
                    tenant_id = %tenant_id,
                    session_id = %session_id,
                    error = %e,
                    "failed to persist session",
                );
            }
        }));
    }

    #[allow(dead_code)] // Called by SessionActor after delegation arrives in Task 5/6
    pub fn persist_status(&self, status: &str) {
        let Some(ref store) = self.store else { return; };
        let store = store.clone();
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();
        let status = status.to_string();
        tokio::spawn(async move {
            if let Err(e) = store.update_session_status(&tenant_id, &session_id, &status).await {
                tracing::warn!(
                    %tenant_id,
                    %session_id,
                    %status,
                    error = %e,
                    "failed to persist session status",
                );
            }
        });
    }

    pub async fn flush(&mut self) -> Result<(), AgentError> {
        if let Some(handle) = self.last_save.take() {
            let _ = handle.await;
        }
        if let Some(ref store) = self.store {
            store.save_session(&self.tenant_id, &self.session_id, &self.entries).await?;
            tracing::info!(
                tenant_id = %self.tenant_id,
                session_id = %self.session_id,
                "session state flushed to store",
            );
        }
        Ok(())
    }

    /// Take the in-flight save join handle (used by flush / shutdown).
    #[allow(dead_code)]
    pub(crate) fn take_last_save(&mut self) -> Option<JoinHandle<()>> {
        self.last_save.take()
    }

    /// Append a compaction entry directly (used by auto-compaction).
    #[allow(dead_code)]
    pub fn append_compaction_entry(&mut self, entry: SessionEntry) {
        self.entries.push(entry);
    }

    /// Clear all entries (used by reset / context-clear strategy).
    #[allow(dead_code)]
    pub fn clear_entries(&mut self) {
        self.entries.clear();
    }

    /// Borrow the underlying entries (used by compaction hooks).
    #[allow(dead_code)]
    pub fn entries_clone(&self) -> Vec<SessionEntry> {
        self.entries.clone()
    }

    // ── Test-only accessor (compat with test_entries_api_with_compaction's field access) ──

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn entries_mut(&mut self) -> &mut Vec<SessionEntry> {
        &mut self.entries
    }

    // ── Capacity ──

    pub fn estimate_tokens(&self) -> usize {
        estimate_context_tokens(&self.entries)
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::entry::SessionEntry;
    use crate::types::AgentMessage;
    use ai_provider::{AssistantMessage, Content, StopReason, Usage};

    fn msg(text: &str) -> AgentMessage {
        AgentMessage::Assistant(AssistantMessage {
            content: vec![Content::Text { text: text.to_string(), text_signature: None }],
            provider: "test".to_string(),
            model: "test".to_string(),
            api: ai_provider::Api {
                provider: "test".to_string(),
                model: "test".to_string(),
            },
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 0,
            },
            stop_reason: StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: SystemTime::now(),
        })
    }

    #[tokio::test]
    async fn history_push_and_messages() {
        let mut h = SessionHistory::new("t1", "s1", None);
        assert!(h.is_empty());
        h.push(msg("hello"));
        h.push(msg("world"));
        assert_eq!(h.len(), 2);
        assert_eq!(h.messages().len(), 2);
    }

    #[tokio::test]
    async fn history_auto_restore_empty_store() {
        let mut h = SessionHistory::new("t1", "s1", None);
        h.auto_restore().await.unwrap();
        assert!(h.is_empty());
    }

    #[tokio::test]
    async fn history_auto_restore_resets_needs_restore_on_success() {
        // After auto_restore() returns Ok, subsequent calls are no-ops (no double-fetch).
        let mut h = SessionHistory::new("t1", "s1", None);
        h.auto_restore().await.unwrap();
        h.auto_restore().await.unwrap();
        assert!(h.is_empty());
    }

    #[tokio::test]
    async fn history_estimate_tokens_via_compactor() {
        let mut h = SessionHistory::new("t1", "s1", None);
        h.push(msg("hello world this is a test"));
        assert!(h.estimate_tokens() > 0);
    }

    #[test]
    fn history_truncate_before_removes_strictly_older_entries() {
        let mut h = SessionHistory::new("t1", "s1", None);
        h.push(msg("a"));
        h.push(msg("b"));
        // Capture the ID of "b" (the boundary entry to keep).
        let b_id = match h.entries().last().unwrap() {
            SessionEntry::Message { id, .. } => *id,
            _ => panic!("expected Message entry"),
        };
        h.push(msg("c"));
        // After truncate_before(b_id), entries strictly before "b" are removed.
        // "a" is removed; "b" and "c" remain.
        h.truncate_before(b_id);
        assert_eq!(h.len(), 2, "should keep 'b' and 'c'");
    }

    #[test]
    fn history_truncate_before_handles_missing_boundary_gracefully() {
        let mut h = SessionHistory::new("t1", "s1", None);
        h.push(msg("a"));
        // Nonexistent boundary is a no-op.
        h.truncate_before(uuid::Uuid::new_v4());
        assert_eq!(h.len(), 1);
    }
}