use crate::cache::CacheRetention;
use crate::error::LlmError;
use crate::streaming::AssistantMessageEvent;
use crate::types::{Api, AssistantMessage, StopReason, ToolCall};

#[derive(Clone, Debug)]
pub enum BlockType {
    Text,
    ToolUse(String, String),
    Thinking,
    RedactedThinking,
}

/// State machine for parsing Anthropic Messages API streaming events.
pub struct StreamParser {
    pub partial: AssistantMessage,
    pub content_index: usize,
    pub current_block: Option<BlockType>,
    pub text_accum: String,
    pub tool_accum: String,
    pub thinking_accum: String,
    pub thinking_signature: Option<String>,
}

impl StreamParser {
    pub fn new(provider: &str, model: &str) -> Self {
        Self {
            partial: AssistantMessage {
                content: vec![],
                provider: provider.to_string(),
                model: model.to_string(),
                api: Api {
                    provider: provider.to_string(),
                    model: model.to_string(),
                },
                usage: crate::Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: crate::StopReason::Stop,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            },
            content_index: 0,
            current_block: None,
            text_accum: String::new(),
            tool_accum: String::new(),
            thinking_accum: String::new(),
            thinking_signature: None,
        }
    }

    /// Process a single stream event (parsed JSON).
    /// Returns `Ok(Some(stop_reason))` when `message_stop` is reached.
    pub async fn process_event(
        &mut self,
        event: &serde_json::Value,
        tx: &tokio::sync::mpsc::Sender<AssistantMessageEvent>,
    ) -> Result<Option<StopReason>, LlmError> {
        let ev_type = event["type"].as_str().unwrap_or("");
        match ev_type {
            "message_start" => {
                if let Some(msg) = event["message"].as_object() {
                    if let Some(id) = msg.get("id").and_then(|v| v.as_str()) {
                        self.partial.response_id = Some(id.to_string());
                    }
                    if let Some(u) = msg.get("usage") {
                        self.partial.usage.input_tokens =
                            u["input_tokens"].as_u64().unwrap_or(0);
                        self.partial.usage.output_tokens =
                            u["output_tokens"].as_u64().unwrap_or(0);
                        self.partial.usage.total_tokens = self.partial.usage.compute_total();
                    }
                }
            }
            "content_block_start" => {
                let block = &event["content_block"];
                let block_type = block["type"].as_str().unwrap_or("");
                match block_type {
                    "text" => {
                        self.current_block = Some(BlockType::Text);
                        self.text_accum.clear();
                        let _ = tx
                            .send(AssistantMessageEvent::TextStart {
                                content_index: self.content_index,
                                partial: self.partial.clone(),
                            })
                            .await;
                    }
                    "tool_use" => {
                        let id = block["id"].as_str().unwrap_or("").to_string();
                        let name = block["name"].as_str().unwrap_or("").to_string();
                        self.current_block = Some(BlockType::ToolUse(id, name));
                        self.tool_accum.clear();
                        let _ = tx
                            .send(AssistantMessageEvent::ToolCallStart {
                                content_index: self.content_index,
                                partial: self.partial.clone(),
                            })
                            .await;
                    }
                    "thinking" => {
                        self.current_block = Some(BlockType::Thinking);
                        self.thinking_accum.clear();
                        self.thinking_signature = None;
                        let _ = tx
                            .send(AssistantMessageEvent::ThinkingStart {
                                content_index: self.content_index,
                                partial: self.partial.clone(),
                            })
                            .await;
                    }
                    "redacted_thinking" => {
                        self.current_block = Some(BlockType::RedactedThinking);
                        self.thinking_accum.clear();
                        self.thinking_signature = None;
                        let _ = tx
                            .send(AssistantMessageEvent::ThinkingStart {
                                content_index: self.content_index,
                                partial: self.partial.clone(),
                            })
                            .await;
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let delta = &event["delta"];
                let delta_type = delta["type"].as_str().unwrap_or("");
                match delta_type {
                    "text_delta" => {
                        let text = delta["text"].as_str().unwrap_or("");
                        self.text_accum.push_str(text);
                        let _ = tx
                            .send(AssistantMessageEvent::TextDelta {
                                content_index: self.content_index,
                                delta: text.to_string(),
                                partial: self.partial.clone(),
                            })
                            .await;
                    }
                    "input_json_delta" => {
                        let json = delta["partial_json"].as_str().unwrap_or("");
                        self.tool_accum.push_str(json);
                        let _ = tx
                            .send(AssistantMessageEvent::ToolCallDelta {
                                content_index: self.content_index,
                                delta: json.to_string(),
                                partial: self.partial.clone(),
                            })
                            .await;
                    }
                    "thinking_delta" => {
                        let text = delta["thinking"].as_str().unwrap_or("");
                        self.thinking_accum.push_str(text);
                        let _ = tx
                            .send(AssistantMessageEvent::ThinkingDelta {
                                content_index: self.content_index,
                                delta: text.to_string(),
                                partial: self.partial.clone(),
                            })
                            .await;
                    }
                    "signature_delta" => {
                        if let Some(sig) = delta["signature"].as_str() {
                            let s = format!(
                                "{}{}",
                                self.thinking_signature.as_deref().unwrap_or(""),
                                sig
                            );
                            self.thinking_signature = Some(s);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                match &self.current_block {
                    Some(BlockType::Text) => {
                        let text = std::mem::take(&mut self.text_accum);
                        let _ = tx
                            .send(AssistantMessageEvent::TextEnd {
                                content_index: self.content_index,
                                text,
                                partial: self.partial.clone(),
                            })
                            .await;
                    }
                    Some(BlockType::ToolUse(id, name)) => {
                        if let Ok(args) = serde_json::from_str(&self.tool_accum) {
                            let tc = ToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                arguments: args,
                                thought_signature: None,
                            };
                            self.partial
                                .content
                                .push(crate::Content::ToolCall(tc.clone()));
                            let _ = tx
                                .send(AssistantMessageEvent::ToolCallEnd {
                                    content_index: self.content_index,
                                    tool_call: tc,
                                    partial: self.partial.clone(),
                                })
                                .await;
                        }
                    }
                    Some(BlockType::Thinking) | Some(BlockType::RedactedThinking) => {
                        let thinking = std::mem::take(&mut self.thinking_accum);
                        let sig = std::mem::take(&mut self.thinking_signature);
                        let redacted = matches!(&self.current_block, Some(BlockType::RedactedThinking));
                        self.partial.content.push(crate::Content::Thinking {
                            thinking: thinking.clone(),
                            thinking_signature: sig,
                            redacted,
                        });
                        let _ = tx
                            .send(AssistantMessageEvent::ThinkingEnd {
                                content_index: self.content_index,
                                thinking,
                                partial: self.partial.clone(),
                            })
                            .await;
                    }
                    None => {}
                }
                self.current_block = None;
                self.content_index += 1;
            }
            "message_delta" => {
                if let Some(delta) = event["delta"].as_object()
                    && let Some(sr) = delta.get("stop_reason").and_then(|v| v.as_str())
                {
                    self.partial.stop_reason = match sr {
                        "end_turn" => StopReason::Stop,
                        "max_tokens" => StopReason::Length,
                        "tool_use" => StopReason::ToolUse,
                        "refusal" => StopReason::Error,
                        _ => StopReason::Stop,
                    };
                }
                if let Some(u) = event["usage"].as_object() {
                    self.partial.usage.output_tokens =
                        u["output_tokens"].as_u64().unwrap_or(self.partial.usage.output_tokens);
                    self.partial.usage.total_tokens = self.partial.usage.compute_total();
                }
            }
            "message_stop" => {
                let _ = tx
                    .send(AssistantMessageEvent::Done {
                        reason: self.partial.stop_reason.clone(),
                        message: self.partial.clone(),
                    })
                    .await;
                return Ok(Some(self.partial.stop_reason.clone()));
            }
            _ => {}
        }
        Ok(None)
    }
}

/// Build the `messages` JSON array from LlmContext messages.
pub fn build_messages_json(messages: &[crate::Message]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(|msg| match msg {
            crate::Message::User(m) => serde_json::json!({
                "role": "user",
                "content": m.content.iter().map(|c| match c {
                    crate::Content::Text { text, .. } => serde_json::json!({"type": "text", "text": text}),
                    crate::Content::Image { data, mime_type } => serde_json::json!({"type": "image", "source": {"type": "base64", "media_type": mime_type, "data": data}}),
                    _ => serde_json::json!({"type": "text", "text": ""}),
                }).collect::<Vec<_>>(),
            }),
            crate::Message::Assistant(m) => serde_json::json!({
                "role": "assistant",
                "content": m.content.iter().map(|c| match c {
                    crate::Content::Text { text, .. } => serde_json::json!({"type": "text", "text": text}),
                    crate::Content::ToolCall(tc) => serde_json::json!({"type": "tool_use", "id": tc.id, "name": tc.name, "input": tc.arguments}),
                    crate::Content::Thinking { thinking, thinking_signature, .. } => {
                        let mut block = serde_json::json!({"type": "thinking", "thinking": thinking});
                        if let Some(sig) = thinking_signature {
                            block["signature"] = serde_json::json!(sig);
                        }
                        block
                    },
                    _ => serde_json::json!({"type": "text", "text": ""}),
                }).collect::<Vec<_>>(),
            }),
            crate::Message::ToolResult(m) => serde_json::json!({
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": m.tool_call_id, "content": m.content.iter().map(|c| match c {
                    crate::Content::Text { text, .. } => serde_json::json!({"type": "text", "text": text}),
                    _ => serde_json::json!({"type": "text", "text": ""}),
                }).collect::<Vec<_>>()}],
            }),
        })
        .collect()
}

/// Build cache_control JSON value.
pub fn build_cache_control(retention: CacheRetention) -> Option<serde_json::Value> {
    match retention {
        CacheRetention::None => None,
        CacheRetention::Short => Some(serde_json::json!({"type": "ephemeral"})),
        CacheRetention::Long => {
            Some(serde_json::json!({"type": "ephemeral", "ttl": "1h"}))
        }
    }
}

/// Build system prompt blocks with optional cache control.
pub fn build_system_blocks(
    system_prompt: &str,
    cache_retention: CacheRetention,
) -> Vec<serde_json::Value> {
    let cache_control = build_cache_control(cache_retention);
    let mut blocks = vec![serde_json::json!({"type": "text", "text": system_prompt})];
    if let Some(cc) = &cache_control {
        for block in &mut blocks {
            block["cache_control"] = serde_json::json!(cc);
        }
    }
    blocks
}

/// Build tools JSON array with optional cache control on last tool.
pub fn build_tools_json(
    tools: &[crate::ToolDef],
    cache_retention: CacheRetention,
) -> Vec<serde_json::Value> {
    let cache_control = build_cache_control(cache_retention);
    let mut tool_json: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters,
            })
        })
        .collect();
    if let (Some(cc), Some(last)) = (cache_control, tool_json.last_mut()) {
        last["cache_control"] = serde_json::json!(cc);
    }
    tool_json
}

/// Apply cache_control to the last user message's last content block.
pub fn apply_cache_to_last_user_message(messages_json: &mut [serde_json::Value], retention: CacheRetention) {
    if let Some(cc) = build_cache_control(retention)
        && let Some(last_user_msg) = messages_json.iter_mut().rev().find(|m| m["role"].as_str() == Some("user"))
        && let Some(content) = last_user_msg["content"].as_array_mut()
        && let Some(last_block) = content.last_mut()
    {
        last_block["cache_control"] = serde_json::json!(cc);
    }
}

/// Check if model uses adaptive thinking.
pub fn is_adaptive_model(model_id: &str) -> bool {
    model_id.contains("opus-4-6")
        || model_id.contains("opus-4.6")
        || model_id.contains("opus-4-7")
        || model_id.contains("opus-4.7")
        || model_id.contains("sonnet-4-6")
        || model_id.contains("sonnet-4.6")
        || model_id.contains("haiku-4-7")
}

/// Map reasoning level to effort string for adaptive models.
pub fn map_effort(
    level: crate::provider::ReasoningLevel,
    model_id: &str,
) -> &'static str {
    let is_opus47 = model_id.contains("opus-4-7") || model_id.contains("opus-4.7");
    match level {
        crate::provider::ReasoningLevel::Minimal => "low",
        crate::provider::ReasoningLevel::Low => "low",
        crate::provider::ReasoningLevel::Medium => "medium",
        crate::provider::ReasoningLevel::High => "high",
        crate::provider::ReasoningLevel::XHigh => {
            if is_opus47 {
                "xhigh"
            } else {
                "high"
            }
        }
    }
}

/// Thinking configuration variants.
#[derive(Debug, Clone, PartialEq)]
pub enum ThinkingConfig {
    Disabled,
    Enabled { budget_tokens: u32 },
    Adaptive { effort: &'static str },
}

/// Build thinking/reasoning configuration.
/// Returns `(new_max_tokens, thinking_config)`.
pub fn build_thinking_config(
    reasoning: Option<crate::provider::ReasoningLevel>,
    model_id: &str,
    max_tokens: u32,
    thinking_budgets: Option<&crate::provider::ThinkingBudgets>,
) -> (u32, ThinkingConfig) {
    let level = match reasoning {
        Some(l) => l,
        None => return (max_tokens, ThinkingConfig::Disabled),
    };

    if is_adaptive_model(model_id) {
        let effort = map_effort(level, model_id);
        return (max_tokens, ThinkingConfig::Adaptive { effort });
    }

    let (new_max, budget) = crate::provider::adjust_max_tokens_for_thinking(
        max_tokens,
        max_tokens.max(16384),
        level,
        thinking_budgets,
    );
    (new_max, ThinkingConfig::Enabled { budget_tokens: budget })
}
