use std::pin::Pin;

use crate::types::{Api, Content, StopReason, ToolCall, Usage};

#[derive(Debug, Clone)]
pub enum AssistantMessageEvent {
    Start,
    TextDelta { text: String },
    ToolCallDelta { tool_call: ToolCall },
    Done {
        content: Vec<Content>,
        api: Api,
        usage: Usage,
        stop_reason: StopReason,
    },
    Error { message: String },
}

pub type AssistantMessageEventStream =
    Pin<Box<dyn futures::Stream<Item = Result<AssistantMessageEvent, crate::error::LlmError>> + Send>>;
