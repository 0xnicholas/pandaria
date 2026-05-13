use std::time::SystemTime;
use uuid::Uuid;

use crate::types::AgentMessage;

/// A single entry in the session history.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SessionEntry {
    /// A standard message (user, assistant, tool result)
    Message {
        id: Uuid,
        message: AgentMessage,
    },
    /// A compaction boundary — marks where old messages were summarized.
    /// Entries before this boundary are not sent to LLM context.
    Compaction {
        id: Uuid,
        summary: String,
        first_kept_entry_id: Uuid,
        tokens_before: usize,
        details: Option<CompactionDetails>,
        from_extension: bool,
        timestamp: SystemTime,
    },
}

impl SessionEntry {
    pub fn id(&self) -> Uuid {
        match self {
            SessionEntry::Message { id, .. } => *id,
            SessionEntry::Compaction { id, .. } => *id,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompactionDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// Builds AgentMessage context from entries for LLM consumption.
/// - Skips entries before the last compaction boundary
/// - Injects compaction summary as the first message (system-like)
pub struct SessionContextBuilder;

impl SessionContextBuilder {
    pub fn build_context(entries: &[SessionEntry]) -> Vec<AgentMessage> {
        // Find last compaction boundary
        let last_compaction_idx = entries
            .iter()
            .rposition(|e| matches!(e, SessionEntry::Compaction { .. }));
        let start_idx = last_compaction_idx.map(|i| i + 1).unwrap_or(0);

        let mut messages = Vec::new();

        // Inject compaction summary if exists
        if let Some(SessionEntry::Compaction { summary, .. }) =
            last_compaction_idx.map(|i| &entries[i])
        {
            messages.push(AgentMessage::User(ai_provider::UserMessage {
                content: vec![ai_provider::Content::Text {
                    text: format!("[Context Summary]\n{}", summary),
                    text_signature: None,
                }],
                timestamp: SystemTime::now(),
            }));
        }

        // Collect messages after boundary, excluding error assistant messages
        // (error messages are kept in entries for transcript but not sent to LLM)
        for entry in &entries[start_idx..] {
            if let SessionEntry::Message { message: msg, .. } = entry {
                if let AgentMessage::Assistant(assistant) = msg
                    && assistant.stop_reason == ai_provider::StopReason::Error
                {
                    continue;
                }
                messages.push(msg.clone());
            }
        }

        messages
    }
}
