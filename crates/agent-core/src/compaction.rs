use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::error::AgentError;
use crate::types::AgentMessage;

// ============================================================================
// Settings
// ============================================================================

/// Configuration for automatic context compaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
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
    pub first_kept_entry_id: u64,
    pub tokens_before: u64,
    pub details: Option<serde_json::Value>,
}

// ============================================================================
// File operation tracking
// ============================================================================

/// Accumulates file operations observed across the messages being compacted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileOperations {
    pub read: HashSet<String>,
    pub written: HashSet<String>,
    pub edited: HashSet<String>,
}

impl FileOperations {
    /// Merge another `FileOperations` into this one.
    pub fn merge(&mut self, other: &FileOperations) {
        self.read.extend(other.read.iter().cloned());
        self.written.extend(other.written.iter().cloned());
        self.edited.extend(other.edited.iter().cloned());
    }
}

/// Computes the final file lists from accumulated operations.
pub fn compute_file_lists(ops: &FileOperations) -> (Vec<String>, Vec<String>) {
    let mut modified: HashSet<String> =
        ops.edited.union(&ops.written).cloned().collect();
    let read_only: Vec<String> = ops
        .read
        .difference(&modified)
        .cloned()
        .collect::<Vec<_>>();
    let modified_files: Vec<String> = {
        let mut v: Vec<_> = modified.drain().collect();
        v.sort();
        v
    };
    (read_only, modified_files)
}

/// Formats read / modified file lists as XML tags appended to a summary.
pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut out = String::new();
    if !read_files.is_empty() {
        out.push_str("\n\n<read-files>\n");
        for f in read_files {
            out.push_str(f);
            out.push('\n');
        }
        out.push_str("</read-files>");
    }
    if !modified_files.is_empty() {
        out.push_str("\n\n<modified-files>\n");
        for f in modified_files {
            out.push_str(f);
            out.push('\n');
        }
        out.push_str("</modified-files>");
    }
    out
}

/// Attempts to extract file operations from a single message.
pub fn extract_file_ops_from_message(msg: &AgentMessage, ops: &mut FileOperations) {
    if let AgentMessage::Assistant(assistant) = msg {
        for block in &assistant.content {
            if let llm_client::Content::ToolCall(tc) = block {
                let path = tc
                    .arguments
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                if let Some(path) = path {
                    match tc.name.as_str() {
                        "read" => {
                            ops.read.insert(path);
                        }
                        "write" => {
                            ops.written.insert(path);
                        }
                        "edit" => {
                            ops.edited.insert(path);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

// ============================================================================
// Prompts
// ============================================================================

/// System prompt used for summarization.
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a context summarization assistant. Your task is to read a conversation between a user and an AI coding assistant, then produce a structured summary following the exact format specified.
Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary."#;

/// Initial summarization prompt.
pub const SUMMARIZATION_PROMPT: &str = r#"The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or "(none)" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;

/// Prompt used when a previous summary exists (iterative update).
pub const UPDATE_SUMMARIZATION_PROMPT: &str = r#"The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;

/// Prompt for summarizing the prefix of a split turn.
pub const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = r#"This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix:

## Original Request
[What did the user ask for in this turn?]

## Early Progress
- [Key decisions and work done in the prefix]

## Context for Suffix
- [Information needed to understand the retained recent work]

Be concise. Focus on what's needed to understand the kept suffix."#;

// ============================================================================
// Token estimation
// ============================================================================

/// Estimate token count for a message using chars/4 heuristic.
/// This is intentionally conservative (overestimates).
pub fn estimate_tokens(msg: &AgentMessage) -> u64 {
    let chars = match msg {
        AgentMessage::User(user) => user.content.iter().map(|c| match c {
            llm_client::Content::Text { text, .. } => text.len(),
            llm_client::Content::Image { .. } => 4800, // ~1200 tokens
            llm_client::Content::Thinking { thinking, .. } => thinking.len(),
            llm_client::Content::ToolCall(tc) => {
                tc.name.len() + serde_json::to_string(&tc.arguments).unwrap_or_default().len()
            }
        }).sum::<usize>(),
        AgentMessage::Assistant(assistant) => assistant.content.iter().map(|c| match c {
            llm_client::Content::Text { text, .. } => text.len(),
            llm_client::Content::Image { .. } => 4800,
            llm_client::Content::Thinking { thinking, .. } => thinking.len(),
            llm_client::Content::ToolCall(tc) => {
                tc.name.len() + serde_json::to_string(&tc.arguments).unwrap_or_default().len()
            }
        }).sum::<usize>(),
        AgentMessage::ToolResult(tr) => tr.content.iter().map(|c| match c {
            llm_client::Content::Text { text, .. } => text.len(),
            llm_client::Content::Image { .. } => 4800,
            llm_client::Content::Thinking { .. } => 0,
            llm_client::Content::ToolCall(_) => 0,
        }).sum::<usize>(),
    };
    ((chars + 3) / 4) as u64
}

/// Context token estimate using the last assistant usage as baseline.
pub struct ContextUsageEstimate {
    pub tokens: u64,
    pub usage_tokens: u64,
    pub trailing_tokens: u64,
    pub last_usage_index: Option<usize>,
}

/// Estimate total context tokens from a list of messages.
pub fn estimate_context_tokens(messages: &[AgentMessage]) -> ContextUsageEstimate {
    // Find last assistant message with valid usage
    let last_usage = messages.iter().enumerate().rev().find_map(|(idx, msg)| {
        if let AgentMessage::Assistant(assistant) = msg {
            if matches!(assistant.stop_reason, llm_client::StopReason::Error | llm_client::StopReason::Aborted) {
                return None;
            }
            let usage = assistant.usage.compute_total();
            if usage > 0 {
                return Some((idx, usage));
            }
        }
        None
    });

    if let Some((idx, usage_tokens)) = last_usage {
        let trailing_tokens: u64 = messages.iter().skip(idx + 1).map(estimate_tokens).sum();
        ContextUsageEstimate {
            tokens: usage_tokens + trailing_tokens,
            usage_tokens,
            trailing_tokens,
            last_usage_index: Some(idx),
        }
    } else {
        let estimated: u64 = messages.iter().map(estimate_tokens).sum();
        ContextUsageEstimate {
            tokens: estimated,
            usage_tokens: 0,
            trailing_tokens: estimated,
            last_usage_index: None,
        }
    }
}

/// Check if compaction should trigger.
pub fn should_compact(context_tokens: u64, context_window: u64, settings: &CompactionSettings) -> bool {
    if !settings.enabled {
        return false;
    }
    context_tokens > context_window.saturating_sub(settings.reserve_tokens)
}

// ============================================================================
// Cut point detection
// ============================================================================

/// Result of finding a cut point.
pub struct CutPointResult {
    /// Index of first entry to keep (in entries slice).
    pub first_kept_index: usize,
    /// If splitting a turn, index of the turn start.
    pub turn_start_index: Option<usize>,
    /// Whether this cut splits a turn.
    pub is_split_turn: bool,
}

/// Find valid cut points (user/assistant messages, never tool results).
fn find_valid_cut_points(messages: &[AgentMessage]) -> Vec<usize> {
    messages.iter().enumerate().filter_map(|(idx, msg)| {
        match msg {
            AgentMessage::User(_) | AgentMessage::Assistant(_) => Some(idx),
            AgentMessage::ToolResult(_) => None,
        }
    }).collect()
}

/// Find the user message that starts the turn containing the given index.
fn find_turn_start(messages: &[AgentMessage], entry_index: usize) -> Option<usize> {
    for i in (0..=entry_index).rev() {
        if matches!(messages[i], AgentMessage::User(_)) {
            return Some(i);
        }
    }
    None
}

/// Find the cut point that keeps approximately `keep_recent_tokens`.
pub fn find_cut_point(
    messages: &[AgentMessage],
    keep_recent_tokens: u64,
) -> CutPointResult {
    let cut_points = find_valid_cut_points(messages);

    if cut_points.is_empty() {
        return CutPointResult {
            first_kept_index: 0,
            turn_start_index: None,
            is_split_turn: false,
        };
    }

    // Walk backwards from newest, accumulating estimated message sizes
    let mut accumulated = 0u64;
    let mut cut_index = 0usize; // Default: keep from first message

    for i in (0..messages.len()).rev() {
        let message_tokens = estimate_tokens(&messages[i]);
        accumulated += message_tokens;

        if accumulated >= keep_recent_tokens {
            // Find the closest valid cut point at or after this entry
            for &cp in &cut_points {
                if cp >= i {
                    cut_index = cp;
                    break;
                }
            }
            break;
        }
    }

    // Determine if this is a split turn
    let is_user_message = matches!(messages[cut_index], AgentMessage::User(_));
    let turn_start_index = if !is_user_message {
        find_turn_start(messages, cut_index)
    } else {
        None
    };

    CutPointResult {
        first_kept_index: cut_index,
        turn_start_index,
        is_split_turn: !is_user_message && turn_start_index.is_some(),
    }
}

// ============================================================================
// Compaction preparation
// ============================================================================

/// Preparation data for compaction.
pub struct CompactionPreparation {
    pub first_kept_entry_id: u64,
    pub messages_to_summarize: Vec<AgentMessage>,
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: u64,
    pub previous_summary: Option<String>,
    pub file_ops: FileOperations,
}

/// Extract a message from a SessionEntry if it produces one.
pub fn message_from_entry(entry: &crate::types::SessionEntry) -> Option<AgentMessage> {
    match &entry.kind {
        crate::types::SessionEntryKind::Message(msg) => Some(msg.clone()),
        crate::types::SessionEntryKind::Compaction(_) => None,
    }
}

/// Prepare compaction data from session entries.
pub fn prepare_compaction(
    entries: &[crate::types::SessionEntry],
    settings: &CompactionSettings,
) -> Option<CompactionPreparation> {
    if entries.is_empty() {
        return None;
    }

    // Find previous compaction
    let prev_compaction_idx = entries.iter().enumerate().rev().find_map(|(idx, entry)| {
        if matches!(entry.kind, crate::types::SessionEntryKind::Compaction(_)) {
            Some(idx)
        } else {
            None
        }
    });

    let (previous_summary, boundary_start) = if let Some(idx) = prev_compaction_idx {
        if let crate::types::SessionEntryKind::Compaction(compaction) = &entries[idx].kind {
            let first_kept = entries.iter().position(|e| e.id == compaction.first_kept_entry_id);
            let start = first_kept.unwrap_or(idx + 1);
            (Some(compaction.summary.clone()), start)
        } else {
            (None, 0)
        }
    } else {
        (None, 0)
    };

    // Extract messages for token estimation
    let messages: Vec<AgentMessage> = entries.iter().filter_map(message_from_entry).collect();
    let tokens_before = estimate_context_tokens(&messages).tokens;

    // Find cut point among the messages (not entries)
    let cut_point = find_cut_point(&messages, settings.keep_recent_tokens);

    // Map message index back to entry index for first_kept_entry_id
    let first_kept_entry_id = entries[cut_point.first_kept_index].id;

    // Determine history end (where to stop collecting messages to summarize)
    let history_end = if cut_point.is_split_turn {
        cut_point.turn_start_index.unwrap_or(cut_point.first_kept_index)
    } else {
        cut_point.first_kept_index
    };

    // Messages to summarize
    let messages_to_summarize: Vec<AgentMessage> = entries
        .iter()
        .skip(boundary_start)
        .take(history_end.saturating_sub(boundary_start))
        .filter_map(message_from_entry)
        .collect();

    // Turn prefix messages (if splitting)
    let turn_prefix_messages: Vec<AgentMessage> = if cut_point.is_split_turn {
        let start = cut_point.turn_start_index.unwrap_or(0);
        entries
            .iter()
            .skip(start)
            .take(cut_point.first_kept_index.saturating_sub(start))
            .filter_map(message_from_entry)
            .collect()
    } else {
        Vec::new()
    };

    // Extract file operations
    let mut file_ops = FileOperations::default();
    for msg in &messages_to_summarize {
        extract_file_ops_from_message(msg, &mut file_ops);
    }
    if cut_point.is_split_turn {
        for msg in &turn_prefix_messages {
            extract_file_ops_from_message(msg, &mut file_ops);
        }
    }

    // Also merge previous compaction's file ops
    if let Some(idx) = prev_compaction_idx {
        if let crate::types::SessionEntryKind::Compaction(compaction) = &entries[idx].kind {
            if let Some(details) = &compaction.details {
                if let Ok(prev_ops) = serde_json::from_value::<FileOperations>(details.clone()) {
                    file_ops.merge(&prev_ops);
                }
            }
        }
    }

    Some(CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        turn_prefix_messages,
        is_split_turn: cut_point.is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
    })
}

// ============================================================================
// Message serialization
// ============================================================================

const TRUNCATE_LEN: usize = 2000;

/// Serialize a conversation to plain text for summarization.
///
/// Format:
///   [User]: message text
///   [Assistant]: response text
///   [Assistant tool calls]: tool_name(arg1=val1)
///   [Tool result]: output (truncated)
pub fn serialize_conversation(messages: &[AgentMessage]) -> String {
    let mut out = String::new();
    for msg in messages {
        match msg {
            AgentMessage::User(user) => {
                let text = extract_text(&user.content);
                out.push_str("[User]: ");
                out.push_str(&text);
                out.push('\n');
            }
            AgentMessage::Assistant(assistant) => {
                for block in &assistant.content {
                    match block {
                        llm_client::Content::Text { text, .. } => {
                            out.push_str("[Assistant]: ");
                            out.push_str(text);
                            out.push('\n');
                        }
                        llm_client::Content::Thinking { thinking, .. } => {
                            out.push_str("[Assistant thinking]: ");
                            out.push_str(thinking);
                            out.push('\n');
                        }
                        llm_client::Content::ToolCall(tc) => {
                            out.push_str("[Assistant tool calls]: ");
                            out.push_str(&tc.name);
                            out.push('(');
                            out.push_str(&serde_json::to_string(&tc.arguments).unwrap_or_default());
                            out.push_str(")\n");
                        }
                        llm_client::Content::Image { .. } => {}
                    }
                }
            }
            AgentMessage::ToolResult(tr) => {
                let text = extract_text(&tr.content);
                let display = if text.len() > TRUNCATE_LEN {
                    let truncated = &text[..TRUNCATE_LEN];
                    format!("{}\n[... {} more characters truncated]", truncated, text.len() - TRUNCATE_LEN)
                } else {
                    text
                };
                out.push_str("[Tool result]: ");
                out.push_str(&display);
                out.push('\n');
            }
        }
    }
    out
}

fn extract_text(content: &[llm_client::Content]) -> String {
    content.iter().filter_map(|c| match c {
        llm_client::Content::Text { text, .. } => Some(text.as_str()),
        _ => None,
    }).collect::<Vec<_>>().join("")
}

// ============================================================================
// Summary generation
// ============================================================================

use llm_client::{LlmContext, LlmProvider, StreamOptions};
use tokio_util::sync::CancellationToken;

/// Generate a summary of the conversation using the LLM.
///
/// If `previous_summary` is provided, uses the update prompt for iterative
/// refinement.  `max_tokens` is set to `floor(0.8 * reserve_tokens)`.
pub async fn generate_summary(
    messages: &[AgentMessage],
    provider: &dyn LlmProvider,
    model: &str,
    reserve_tokens: u64,
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
    signal: CancellationToken,
) -> Result<String, AgentError> {
    let max_tokens = ((reserve_tokens as f64) * 0.8).floor() as u32;

    let base_prompt = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT
    } else {
        SUMMARIZATION_PROMPT
    };

    let mut prompt_text = format!(
        "<conversation>\n{}\n</conversation>\n\n",
        serialize_conversation(messages)
    );

    if let Some(prev) = previous_summary {
        prompt_text.push_str(&format!("<previous-summary>\n{}\n</previous-summary>\n\n", prev));
    }

    prompt_text.push_str(base_prompt);

    if let Some(instructions) = custom_instructions {
        prompt_text.push_str(&format!("\n\nAdditional focus: {}", instructions));
    }

    let ctx = LlmContext {
        system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        messages: vec![AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text {
                text: prompt_text,
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })],
        tools: None,
    };

    let mut options = StreamOptions::default();
    options.max_tokens = Some(max_tokens);

    let mut stream = provider.stream(model, ctx, options, signal).await
        .map_err(|e| AgentError::LlmError(e))?;

    let mut summary = String::new();
    let mut stop_reason = llm_client::StopReason::Stop;

    while let Some(event) = stream.next().await {
        match event {
            llm_client::AssistantMessageEvent::TextDelta { delta, .. } => {
                summary.push_str(&delta);
            }
            llm_client::AssistantMessageEvent::Done { reason, .. } => {
                stop_reason = reason;
                break;
            }
            llm_client::AssistantMessageEvent::Error { error } => {
                return Err(AgentError::LlmResponseError(
                    error.error_message.unwrap_or_else(|| "Summarization failed".to_string())
                ));
            }
            _ => {}
        }
    }

    if matches!(stop_reason, llm_client::StopReason::Error) {
        return Err(AgentError::LlmResponseError("Summarization stopped with error".to_string()));
    }

    Ok(summary)
}

/// Generate a summary for a turn prefix (when splitting a turn).
pub async fn generate_turn_prefix_summary(
    messages: &[AgentMessage],
    provider: &dyn LlmProvider,
    model: &str,
    reserve_tokens: u64,
    signal: CancellationToken,
) -> Result<String, AgentError> {
    let max_tokens = ((reserve_tokens as f64) * 0.5).floor() as u32;

    let prompt_text = format!(
        "<conversation>\n{}\n</conversation>\n\n{}",
        serialize_conversation(messages),
        TURN_PREFIX_SUMMARIZATION_PROMPT
    );

    let ctx = LlmContext {
        system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        messages: vec![AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text {
                text: prompt_text,
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })],
        tools: None,
    };

    let mut options = StreamOptions::default();
    options.max_tokens = Some(max_tokens);

    let mut stream = provider.stream(model, ctx, options, signal).await
        .map_err(|e| AgentError::LlmError(e))?;

    let mut summary = String::new();
    let mut stop_reason = llm_client::StopReason::Stop;

    while let Some(event) = stream.next().await {
        match event {
            llm_client::AssistantMessageEvent::TextDelta { delta, .. } => {
                summary.push_str(&delta);
            }
            llm_client::AssistantMessageEvent::Done { reason, .. } => {
                stop_reason = reason;
                break;
            }
            llm_client::AssistantMessageEvent::Error { error } => {
                return Err(AgentError::LlmResponseError(
                    error.error_message.unwrap_or_else(|| "Turn prefix summarization failed".to_string())
                ));
            }
            _ => {}
        }
    }

    if matches!(stop_reason, llm_client::StopReason::Error) {
        return Err(AgentError::LlmResponseError("Turn prefix summarization stopped with error".to_string()));
    }

    Ok(summary)
}

// ============================================================================
// Main compaction orchestration
// ============================================================================

/// Execute compaction using the prepared data.
///
/// Returns `CompactionResult` which the caller can append to the session
/// entries.
pub async fn compact(
    preparation: &CompactionPreparation,
    provider: &dyn LlmProvider,
    model: &str,
    reserve_tokens: u64,
    custom_instructions: Option<&str>,
    signal: CancellationToken,
) -> Result<CompactionResult, AgentError> {
    let summary = if preparation.is_split_turn && !preparation.turn_prefix_messages.is_empty() {
        // Generate both summaries in parallel
        let (history_result, prefix_result) = if preparation.messages_to_summarize.is_empty() {
            let prefix = generate_turn_prefix_summary(
                &preparation.turn_prefix_messages,
                provider,
                model,
                reserve_tokens,
                signal.child_token(),
            ).await?;
            ("No prior history.".to_string(), prefix)
        } else {
            let history_fut = generate_summary(
                &preparation.messages_to_summarize,
                provider,
                model,
                reserve_tokens,
                preparation.previous_summary.as_deref(),
                custom_instructions,
                signal.child_token(),
            );

            let prefix_fut = generate_turn_prefix_summary(
                &preparation.turn_prefix_messages,
                provider,
                model,
                reserve_tokens,
                signal.child_token(),
            );

            futures::future::try_join(history_fut, prefix_fut).await?
        };
        format!("{}\n\n---\n\n**Turn Context (split turn):**\n\n{}", history_result, prefix_result)
    } else {
        generate_summary(
            &preparation.messages_to_summarize,
            provider,
            model,
            reserve_tokens,
            preparation.previous_summary.as_deref(),
            custom_instructions,
            signal,
        ).await?
    };

    // Append file operations
    let (read_files, modified_files) = compute_file_lists(&preparation.file_ops);
    let summary = summary + &format_file_operations(&read_files, &modified_files);

    Ok(CompactionResult {
        summary,
        first_kept_entry_id: preparation.first_kept_entry_id,
        tokens_before: preparation.tokens_before,
        details: Some(serde_json::to_value(&preparation.file_ops).unwrap_or(serde_json::Value::Null)),
    })
}
