use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::CompactionError;
use crate::file_ops::{FileOperationExtractor, FileOperations};
use crate::persistence::entry::{CompactionDetails, SessionEntry};
use crate::types::AgentMessage;

// ============================================================================
// Config
// ============================================================================

/// Configuration for automatic context compaction.
#[derive(Debug, Clone, Default)]
pub struct CompactionConfig {
    pub enabled: bool,
    pub reserve_tokens: usize,
    pub keep_recent_tokens: usize,
}

impl CompactionConfig {
    pub fn new(enabled: bool, reserve_tokens: usize, keep_recent_tokens: usize) -> Self {
        Self {
            enabled,
            reserve_tokens,
            keep_recent_tokens,
        }
    }
}

// ============================================================================
// Result
// ============================================================================

/// Output of a successful compaction.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: Uuid,
    pub tokens_before: usize,
    pub details: Option<CompactionDetails>,
}

// ============================================================================
// Preparation
// ============================================================================

#[derive(Debug, Clone)]
pub struct CompactionPreparation {
    pub first_kept_entry_id: Uuid,
    pub messages_to_summarize: Vec<AgentMessage>,
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: usize,
    pub previous_summary: Option<String>,
    pub file_ops: FileOperations,
}

// ============================================================================
// Token estimation
// ============================================================================

fn estimate_tokens(message: &AgentMessage) -> usize {
    let chars: usize = match message {
        AgentMessage::User(user) => {
            user.content.iter().map(|c| match c {
                ai_provider::Content::Text { text, .. } => text.len(),
                ai_provider::Content::Image { .. } => 4800,
                _ => 0,
            }).sum()
        }
        AgentMessage::Assistant(assistant) => {
            assistant.content.iter().map(|c| match c {
                ai_provider::Content::Text { text, .. } => text.len(),
                ai_provider::Content::Thinking { thinking, .. } => thinking.len(),
                ai_provider::Content::ToolCall(tc) => {
                    tc.name.len() + serde_json::to_string(&tc.arguments).unwrap_or_default().len()
                }
                ai_provider::Content::Image { .. } => 4800,
            }).sum()
        }
        AgentMessage::ToolResult(result) => {
            result.content.iter().map(|c| match c {
                ai_provider::Content::Text { text, .. } => text.len(),
                ai_provider::Content::Image { .. } => 4800,
                _ => 0,
            }).sum()
        }
    };
    (chars as f64 / 4.0).ceil() as usize
}

pub fn estimate_context_tokens(entries: &[SessionEntry]) -> usize {
    let mut tokens = 0;
    let mut last_usage_tokens: Option<usize> = None;
    let mut last_usage_idx: Option<usize> = None;

    for (i, entry) in entries.iter().enumerate() {
        if let SessionEntry::Message {
            message: AgentMessage::Assistant(assistant),
            ..
        } = entry
            && assistant.stop_reason != ai_provider::StopReason::Aborted
            && assistant.stop_reason != ai_provider::StopReason::Error
            && assistant.usage.compute_total() as usize > 0
        {
            last_usage_tokens = Some(assistant.usage.compute_total() as usize);
            last_usage_idx = Some(i);
        }
    }

    if let Some(usage_tokens) = last_usage_tokens {
        tokens = usage_tokens;
        if let Some(idx) = last_usage_idx {
            for entry in &entries[idx + 1..] {
                if let SessionEntry::Message { message: msg, .. } = entry {
                    tokens += estimate_tokens(msg);
                }
            }
        }
    } else {
        for entry in entries {
            if let SessionEntry::Message { message: msg, .. } = entry {
                tokens += estimate_tokens(msg);
            }
        }
    }

    tokens
}

pub fn should_compact(tokens: usize, window: usize, config: &CompactionConfig) -> bool {
    config.enabled && window > 0 && tokens > window.saturating_sub(config.reserve_tokens)
}

// ============================================================================
// Cut point detection
// ============================================================================

#[derive(Debug)]
struct CutPoint {
    first_kept_entry_index: usize,
    turn_start_index: Option<usize>,
    is_split_turn: bool,
}

fn find_valid_cut_points(entries: &[SessionEntry], start: usize, end: usize) -> Vec<usize> {
    let mut points = Vec::new();
    for (i, entry) in entries.iter().enumerate().skip(start).take(end - start) {
        if let SessionEntry::Message { message: msg, .. } = entry
            && matches!(msg, AgentMessage::User(_) | AgentMessage::Assistant(_))
        {
            points.push(i);
        }
    }
    points
}

fn find_turn_start_index(entries: &[SessionEntry], entry_index: usize, start: usize) -> Option<usize> {
    for (i, entry) in entries.iter().enumerate().skip(start).take(entry_index - start + 1).rev() {
        if let SessionEntry::Message {
            message: AgentMessage::User(_),
            ..
        } = entry
        {
            return Some(i);
        }
    }
    None
}

fn find_cut_point(
    entries: &[SessionEntry],
    start_index: usize,
    end_index: usize,
    keep_recent_tokens: usize,
) -> CutPoint {
    let cut_points = find_valid_cut_points(entries, start_index, end_index);

    if cut_points.is_empty() {
        return CutPoint {
            first_kept_entry_index: start_index,
            turn_start_index: None,
            is_split_turn: false,
        };
    }

    let mut accumulated = 0;
    let mut cut_index = cut_points[0];

    for i in (start_index..end_index).rev() {
        if let SessionEntry::Message { message: msg, .. } = &entries[i] {
            accumulated += estimate_tokens(msg);

            if accumulated >= keep_recent_tokens {
                cut_index = cut_points
                    .iter()
                    .find(|cp| **cp >= i)
                    .copied()
                    .unwrap_or(cut_points[0]);
                break;
            }
        }
    }

    // Absorb adjacent non-message entries (future-proofing)
    if cut_index > start_index {
        match &entries[cut_index - 1] {
            SessionEntry::Compaction { .. } => {}
            SessionEntry::Message { .. } => {}
        }
    }

    let is_user_msg = matches!(
        &entries[cut_index],
        SessionEntry::Message {
            message: AgentMessage::User(_),
            ..
        }
    );

    let turn_start_index = if is_user_msg {
        None
    } else {
        find_turn_start_index(entries, cut_index, start_index)
    };

    CutPoint {
        first_kept_entry_index: cut_index,
        turn_start_index,
        is_split_turn: !is_user_msg && turn_start_index.is_some(),
    }
}

// ============================================================================
// CompactionActor
// ============================================================================

pub struct CompactionActor {
    pub config: CompactionConfig,
    provider: Arc<dyn ai_provider::LlmProvider>,
    model: String,
    file_op_extractor: Arc<dyn FileOperationExtractor>,
}

impl CompactionActor {
    pub fn new(
        config: CompactionConfig,
        provider: Arc<dyn ai_provider::LlmProvider>,
        model: String,
        file_op_extractor: Arc<dyn FileOperationExtractor>,
    ) -> Self {
        Self {
            config,
            provider,
            model,
            file_op_extractor,
        }
    }

    pub fn prepare(
        &self,
        entries: &[SessionEntry],
    ) -> Result<CompactionPreparation, CompactionError> {
        if entries.is_empty() {
            return Err(CompactionError::AlreadyCompacted);
        }

        // 1. If last entry is compaction, skip
        if let Some(SessionEntry::Compaction { .. }) = entries.last() {
            return Err(CompactionError::AlreadyCompacted);
        }

        // 2. Find previous compaction
        let prev_compaction_idx = entries
            .iter()
            .rposition(|e| matches!(e, SessionEntry::Compaction { .. }));
        let mut previous_summary = None;
        let mut boundary_start = 0;

        if let Some(idx) = prev_compaction_idx
            && let SessionEntry::Compaction {
                summary,
                first_kept_entry_id,
                ..
            } = &entries[idx]
        {
            previous_summary = Some(summary.clone());
            boundary_start = entries
                .iter()
                .position(|e| {
                    matches!(e, SessionEntry::Message { id, .. } if id == first_kept_entry_id)
                })
                .unwrap_or(idx + 1);
        }

        let boundary_end = entries.len();
        let tokens_before = estimate_context_tokens(entries);

        // 3. Find cut point
        let cut_point =
            find_cut_point(entries, boundary_start, boundary_end, self.config.keep_recent_tokens);

        // 4. Determine history end
        let history_end = if cut_point.is_split_turn {
            cut_point
                .turn_start_index
                .unwrap_or(cut_point.first_kept_entry_index)
        } else {
            cut_point.first_kept_entry_index
        };

        // 5. Collect messages to summarize
        let mut messages_to_summarize = Vec::new();
        for entry in entries.iter().take(history_end).skip(boundary_start) {
            if let SessionEntry::Message { message: msg, .. } = entry {
                messages_to_summarize.push(msg.clone());
            }
        }

        // 6. Collect turn prefix messages
        let mut turn_prefix_messages = Vec::new();
        if cut_point.is_split_turn {
            let turn_start = cut_point.turn_start_index.expect("is_split_turn guarantees turn_start_index is Some");
            for entry in entries.iter().take(cut_point.first_kept_entry_index).skip(turn_start) {
                if let SessionEntry::Message { message: msg, .. } = entry {
                    turn_prefix_messages.push(msg.clone());
                }
            }
        }

        // 7. Extract file operations
        let file_ops = self
            .file_op_extractor
            .extract(&messages_to_summarize);

        // 8. Get the ID of the first kept message entry
        let first_kept_entry_id = entries[cut_point.first_kept_entry_index]
            .id();

        Ok(CompactionPreparation {
            first_kept_entry_id,
            messages_to_summarize,
            turn_prefix_messages,
            is_split_turn: cut_point.is_split_turn,
            tokens_before,
            previous_summary,
            file_ops,
        })
    }

    pub async fn compact(
        &self,
        entries: &[SessionEntry],
        signal: &CancellationToken,
    ) -> Result<CompactionResult, CompactionError> {
        let preparation = self.prepare(entries)?;

        let summary = if preparation.is_split_turn && !preparation.turn_prefix_messages.is_empty() {
            let max_tokens = (self.config.reserve_tokens as f64 * 0.8) as usize;
            let prefix_max_tokens = (self.config.reserve_tokens as f64 * 0.5) as usize;

            let history_future = generate_history_summary(
                &preparation.messages_to_summarize,
                self.provider.as_ref(),
                &self.model,
                preparation.previous_summary.clone(),
                max_tokens,
                signal.child_token(),
            );

            let prefix_future = generate_turn_prefix_summary(
                &preparation.turn_prefix_messages,
                self.provider.as_ref(),
                &self.model,
                prefix_max_tokens,
                signal.child_token(),
            );

            let (history_result, prefix_result) =
                tokio::try_join!(history_future, prefix_future)?;

            format!(
                "{}\n\n---\n\n**Turn Context (split turn):**\n\n{}",
                history_result, prefix_result
            )
        } else {
            let max_tokens = (self.config.reserve_tokens as f64 * 0.8) as usize;
            generate_history_summary(
                &preparation.messages_to_summarize,
                self.provider.as_ref(),
                &self.model,
                preparation.previous_summary.clone(),
                max_tokens,
                signal.child_token(),
            )
            .await?
        };

        let details = CompactionDetails {
            read_files: preparation.file_ops.read,
            modified_files: preparation.file_ops.written,
        };

        Ok(CompactionResult {
            summary,
            first_kept_entry_id: preparation.first_kept_entry_id,
            tokens_before: preparation.tokens_before,
            details: Some(details),
        })
    }
}

// ============================================================================
// Summary generation
// ============================================================================

const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a conversation summarizer. ..."#;

const SUMMARIZATION_PROMPT: &str = r#"Summarize the conversation above into a structured format:
- Overview
- Progress (Done / In Progress)
- Key Decisions
- Current State
- Next Steps
- Important files and functions mentioned

Be concise but preserve exact file paths, function names, and error messages."#;

const UPDATE_SUMMARIZATION_PROMPT: &str = r#"The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it"#;

const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = r#"This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix. Be concise. Focus on what's needed to understand the kept suffix."#;

fn serialize_messages(messages: &[AgentMessage]) -> String {
    let mut output = String::new();
    for msg in messages {
        let (role, text) = match msg {
            AgentMessage::User(user) => ("User", extract_text(&user.content)),
            AgentMessage::Assistant(assistant) => ("Assistant", extract_text(&assistant.content)),
            AgentMessage::ToolResult(result) => ("Tool", extract_text(&result.content)),
        };
        output.push_str(&format!("[{}]: {}\n\n", role, text));
    }
    output
}

fn extract_text(content: &[ai_provider::Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            ai_provider::Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn generate_history_summary(
    messages: &[AgentMessage],
    provider: &dyn ai_provider::LlmProvider,
    model: &str,
    previous_summary: Option<String>,
    max_tokens: usize,
    signal: CancellationToken,
) -> Result<String, CompactionError> {
    let base_prompt = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT
    } else {
        SUMMARIZATION_PROMPT
    };

    let conversation_text = serialize_messages(messages);

    let mut prompt_text = format!("<conversation>\n{}\n</conversation>\n\n", conversation_text);
    if let Some(prev) = previous_summary {
        prompt_text.push_str(&format!("<previous-summary>\n{}\n</previous-summary>\n\n", prev));
    }
    prompt_text.push_str(base_prompt);

    let llm_messages = vec![ai_provider::Message::User(ai_provider::UserMessage {
        content: vec![ai_provider::Content::Text {
            text: prompt_text,
            text_signature: None,
        }],
        timestamp: std::time::SystemTime::now(),
    })];

    let ctx = ai_provider::LlmContext {
        system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        messages: llm_messages,
        tools: None,
    };

    let mut stream = provider
        .stream(
            model,
            ctx,
            ai_provider::StreamOptions {
                max_tokens: Some(max_tokens as u32),
                ..Default::default()
            },
            signal,
        )
        .await
        .map_err(|e| CompactionError::LlmError(e.to_string()))?;

    let mut summary_text = String::new();

    while let Some(event) = stream.next().await {
        match event {
            ai_provider::AssistantMessageEvent::TextDelta { delta, .. } => {
                summary_text.push_str(&delta);
            }
            ai_provider::AssistantMessageEvent::Done { message, .. } => {
                summary_text = message
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ai_provider::Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                break;
            }
            ai_provider::AssistantMessageEvent::Error { error } => {
                let msg = error.error_message.unwrap_or_else(|| "LLM error".to_string());
                return Err(CompactionError::LlmError(msg));
            }
            _ => {}
        }
    }

    if summary_text.is_empty() {
        return Err(CompactionError::LlmError(
            "Summary generation returned empty text".into(),
        ));
    }

    Ok(summary_text)
}

async fn generate_turn_prefix_summary(
    messages: &[AgentMessage],
    provider: &dyn ai_provider::LlmProvider,
    model: &str,
    max_tokens: usize,
    signal: CancellationToken,
) -> Result<String, CompactionError> {
    let conversation_text = serialize_messages(messages);
    let prompt_text = format!(
        "<conversation>\n{}\n</conversation>\n\n{}",
        conversation_text, TURN_PREFIX_SUMMARIZATION_PROMPT
    );

    let llm_messages = vec![ai_provider::Message::User(ai_provider::UserMessage {
        content: vec![ai_provider::Content::Text {
            text: prompt_text,
            text_signature: None,
        }],
        timestamp: std::time::SystemTime::now(),
    })];

    let ctx = ai_provider::LlmContext {
        system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        messages: llm_messages,
        tools: None,
    };

    let mut stream = provider
        .stream(
            model,
            ctx,
            ai_provider::StreamOptions {
                max_tokens: Some(max_tokens as u32),
                ..Default::default()
            },
            signal,
        )
        .await
        .map_err(|e| CompactionError::LlmError(e.to_string()))?;

    let mut summary_text = String::new();

    while let Some(event) = stream.next().await {
        match event {
            ai_provider::AssistantMessageEvent::TextDelta { delta, .. } => {
                summary_text.push_str(&delta);
            }
            ai_provider::AssistantMessageEvent::Done { message, .. } => {
                summary_text = message
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ai_provider::Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                break;
            }
            ai_provider::AssistantMessageEvent::Error { error } => {
                let msg = error.error_message.unwrap_or_else(|| "LLM error".to_string());
                return Err(CompactionError::LlmError(msg));
            }
            _ => {}
        }
    }

    if summary_text.is_empty() {
        return Err(CompactionError::LlmError(
            "Turn prefix summary returned empty text".into(),
        ));
    }

    Ok(summary_text)
}
