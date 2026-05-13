use async_trait::async_trait;

use crate::harness::compaction::CompactionResult;
use crate::hook::context::CompactReason;
use crate::error::AgentError;
use crate::types::AgentMessage;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
    TurnStart {
        turn_index: u64,
    },
    TurnEnd {
        turn_index: u64,
        messages: Vec<AgentMessage>,
    },
    MessageStart {
        message_index: u64,
    },
    MessageUpdate {
        message_index: u64,
        content_delta: String,
    },
    MessageEnd {
        message: AgentMessage,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        content: String,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        result: ai_provider::ToolResultMessage,
    },
    CompactionStart {
        reason: CompactReason,
    },
    CompactionEnd {
        reason: CompactReason,
        result: Option<CompactionResult>,
        aborted: bool,
        will_retry: bool,
        error_message: Option<String>,
    },
    AutoRetryStart {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
    },
    AutoRetryEnd {
        success: bool,
        error: Option<String>,
    },
    Error {
        error: AgentError,
    },
}

#[async_trait]
pub trait AgentEventListener: Send + Sync {
    async fn on_event(&self, event: &AgentEvent);
}
