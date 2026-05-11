use secrecy::{ExposeSecret, SecretString};
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
#[cfg(test)]
use crate::provider::LlmProvider;
use crate::streaming::AssistantMessageEvent;
use crate::types::{Api, LlmContext};

crate::providers::shared::define_provider!(
    AnthropicProvider,
    "anthropic",
    "ANTHROPIC_API_KEY",
    "https://api.anthropic.com/v1/messages"
);

impl AnthropicProvider {
    #[allow(clippy::too_many_arguments)]
    async fn try_stream(
        client: reqwest::Client,
        base_url: String,
        model: &str,
        context: LlmContext,
        options: crate::provider::StreamOptions,
        tx: &tokio::sync::mpsc::Sender<AssistantMessageEvent>,
        api_key: SecretString,
        signal: CancellationToken,
    ) -> Result<(), LlmError> {
        // Build request body per spec §9.1
        let mut messages_json: Vec<serde_json::Value> = Vec::new();
        for msg in &context.messages {
            messages_json.push(match msg {
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
            });
        }

        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": options.max_tokens.unwrap_or(4096),
            "messages": messages_json,
        });

        // System prompt with cache_control
        if let Some(system_prompt) = &context.system_prompt {
            let cache_control = build_cache_control(options.cache_retention);
            let mut system_blocks: Vec<serde_json::Value> = vec![serde_json::json!({
                "type": "text",
                "text": system_prompt,
            })];
            if let Some(cc) = &cache_control {
                for block in &mut system_blocks {
                    block["cache_control"] = serde_json::json!(cc);
                }
            }
            body["system"] = serde_json::json!(system_blocks);
        }

        // Tools with cache_control on last tool
        if let Some(tools) = &context.tools {
            let cache_control = build_cache_control(options.cache_retention);
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
            body["tools"] = serde_json::json!(tool_json);
        }

        // Cache_control on last **user** message's last content block (spec §9.1)
        if let Some(cc) = build_cache_control(options.cache_retention)
            && let Some(last_user_msg) = messages_json
                .iter_mut()
                .rev()
                .find(|m| m["role"].as_str() == Some("user"))
            && let Some(content) = last_user_msg["content"].as_array_mut()
            && let Some(last_block) = content.last_mut()
        {
            last_block["cache_control"] = serde_json::json!(cc);
        }

        // Thinking / reasoning parameters
        if let Some(level) = options.reasoning {
            let model_id = model;
            if is_adaptive_model(model_id) {
                let effort = map_effort(level, model_id);
                body["thinking"] = serde_json::json!({
                    "type": "adaptive",
                    "display": "summarized",
                });
                body["output_config"] = serde_json::json!({
                    "effort": effort,
                });
            } else {
                let (new_max, budget) = crate::provider::adjust_max_tokens_for_thinking(
                    options.max_tokens.unwrap_or(4096),
                    options.max_tokens.unwrap_or(4096).max(16384),
                    level,
                    options.thinking_budgets.as_ref(),
                );
                body["max_tokens"] = serde_json::json!(new_max);
                body["thinking"] = serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": budget,
                    "display": "summarized",
                });
            }
        } else {
            body["thinking"] = serde_json::json!({"type": "disabled"});
        }

        if options.temperature.is_some() {
            body["temperature"] = serde_json::json!(options.temperature);
        }

        // Invoke on_payload hook
        if let Some(hook) = &options.on_payload {
            hook(
                &mut body,
                &crate::Model {
                    id: model.to_string(),
                    name: model.to_string(),
                    api: "anthropic-messages".to_string(),
                    provider: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com/v1/messages".to_string(),
                    reasoning: true,
                    input_modalities: vec![],
                    cost: crate::TokenCost {
                        input: 0.0,
                        output: 0.0,
                        cache_read: 0.0,
                        cache_write: 0.0,
                    },
                    context_window: 200_000,
                    max_tokens: 8192,
                    headers: None,
                    compat: crate::models::ModelCompat::None,
                },
            )
            .await;
        }

        // Send request
        // Merge custom headers from StreamOptions
        let mut req = client
            .post(&base_url)
            .header("x-api-key", api_key.expose_secret())
            .header("anthropic-version", "2023-06-01")
            .header(
                "anthropic-beta",
                "interleaved-thinking-2025-05-14, fine-grained-tool-streaming-2025-05-14",
            )
            .header("content-type", "application/json");
        if let Some(custom) = &options.headers {
            for (k, v) in custom {
                req = req.header(k, v);
            }
        }

        let response = req
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::ProviderError(format!("HTTP error: {e}")))?;

        let status = response.status().as_u16();
        let headers: std::collections::HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        // Invoke on_response hook
        if let Some(hook) = &options.on_response {
            hook(
                &crate::ProviderResponse { status, headers },
                &crate::Model {
                    id: model.to_string(),
                    name: model.to_string(),
                    api: "anthropic-messages".to_string(),
                    provider: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com/v1/messages".to_string(),
                    reasoning: true,
                    input_modalities: vec![],
                    cost: crate::TokenCost {
                        input: 0.0,
                        output: 0.0,
                        cache_read: 0.0,
                        cache_write: 0.0,
                    },
                    context_window: 200_000,
                    max_tokens: 8192,
                    headers: None,
                    compat: crate::models::ModelCompat::None,
                },
            )
            .await;
        }

        if !status.to_string().starts_with('2') {
            let body = response.text().await
                .map_err(|e| LlmError::ProviderError(format!("failed to read response body: {e}")))?;
            return Err(LlmError::ProviderError(format!("HTTP {status}: {body}")));
        }

        // Process SSE stream with full event mapping per spec §9.1
        use futures::StreamExt;
        let mut sse_stream = response.bytes_stream();
        let mut partial = crate::AssistantMessage {
            content: vec![],
            provider: "anthropic".to_string(),
            model: model.to_string(),
            api: Api {
                provider: "anthropic".to_string(),
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
        };

        let _ = tx
            .send(AssistantMessageEvent::Start {
                partial: partial.clone(),
            })
            .await;

        // Track content block state
        let mut content_index: usize = 0;
        #[derive(Clone, Debug)]
        enum BlockType {
            Text,
            ToolUse(String, String),
            Thinking,
            RedactedThinking,
        }
        let mut current_block: Option<BlockType> = None;
        let mut tool_accum: String = String::new();
        let mut text_accum: String = String::new();
        let mut thinking_accum: String = String::new();
        let mut thinking_signature: Option<String> = None;

        let mut buffer = String::new();
        while let Some(chunk) = sse_stream.next().await {
            if signal.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            match chunk {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));
                    while let Some(line_end) = buffer.find('\n') {
                        let line = buffer[..line_end].trim().to_string();
                        buffer = buffer[line_end + 1..].to_string();
                        if let Some(data) = line.strip_prefix("data: ")
                            && let Ok(event) = serde_json::from_str::<serde_json::Value>(data)
                        {
                            let ev_type = event["type"].as_str().unwrap_or("");
                            match ev_type {
                                "message_start" => {
                                    if let Some(msg) = event["message"].as_object() {
                                        if let Some(id) = msg.get("id").and_then(|v| v.as_str()) {
                                            partial.response_id = Some(id.to_string());
                                        }
                                        if let Some(u) = msg.get("usage") {
                                            partial.usage.input_tokens =
                                                u["input_tokens"].as_u64().unwrap_or(0);
                                            partial.usage.output_tokens =
                                                u["output_tokens"].as_u64().unwrap_or(0);
                                            partial.usage.total_tokens =
                                                partial.usage.compute_total();
                                        }
                                    }
                                }
                                "content_block_start" => {
                                    let block = &event["content_block"];
                                    let block_type = block["type"].as_str().unwrap_or("");
                                    match block_type {
                                        "text" => {
                                            current_block = Some(BlockType::Text);
                                            text_accum.clear();
                                            let _ = tx
                                                .send(AssistantMessageEvent::TextStart {
                                                    content_index,
                                                    partial: partial.clone(),
                                                })
                                                .await;
                                        }
                                        "tool_use" => {
                                            let id = block["id"].as_str().unwrap_or("").to_string();
                                            let name =
                                                block["name"].as_str().unwrap_or("").to_string();
                                            current_block = Some(BlockType::ToolUse(id, name));
                                            tool_accum.clear();
                                            let _ = tx
                                                .send(AssistantMessageEvent::ToolCallStart {
                                                    content_index,
                                                    partial: partial.clone(),
                                                })
                                                .await;
                                        }
                                        "thinking" => {
                                            current_block = Some(BlockType::Thinking);
                                            thinking_accum.clear();
                                            thinking_signature = None;
                                            let _ = tx
                                                .send(AssistantMessageEvent::ThinkingStart {
                                                    content_index,
                                                    partial: partial.clone(),
                                                })
                                                .await;
                                        }
                                        "redacted_thinking" => {
                                            current_block = Some(BlockType::RedactedThinking);
                                            thinking_accum.clear();
                                            thinking_signature = None;
                                            let _ = tx
                                                .send(AssistantMessageEvent::ThinkingStart {
                                                    content_index,
                                                    partial: partial.clone(),
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
                                            text_accum.push_str(text);
                                            let _ = tx
                                                .send(AssistantMessageEvent::TextDelta {
                                                    content_index,
                                                    delta: text.to_string(),
                                                    partial: partial.clone(),
                                                })
                                                .await;
                                        }
                                        "input_json_delta" => {
                                            let json = delta["partial_json"].as_str().unwrap_or("");
                                            tool_accum.push_str(json);
                                            let _ = tx
                                                .send(AssistantMessageEvent::ToolCallDelta {
                                                    content_index,
                                                    delta: json.to_string(),
                                                    partial: partial.clone(),
                                                })
                                                .await;
                                        }
                                        "thinking_delta" => {
                                            let text = delta["thinking"].as_str().unwrap_or("");
                                            thinking_accum.push_str(text);
                                            let _ = tx
                                                .send(AssistantMessageEvent::ThinkingDelta {
                                                    content_index,
                                                    delta: text.to_string(),
                                                    partial: partial.clone(),
                                                })
                                                .await;
                                        }
                                        "signature_delta" => {
                                            if let Some(sig) = delta["signature"].as_str() {
                                                let s = format!(
                                                    "{}{}",
                                                    thinking_signature.as_deref().unwrap_or(""),
                                                    sig
                                                );
                                                thinking_signature = Some(s);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                "content_block_stop" => {
                                    match &current_block {
                                        Some(BlockType::Text) => {
                                            let text = std::mem::take(&mut text_accum);
                                            let _ = tx
                                                .send(AssistantMessageEvent::TextEnd {
                                                    content_index,
                                                    text,
                                                    partial: partial.clone(),
                                                })
                                                .await;
                                        }
                                        Some(BlockType::ToolUse(id, name)) => {
                                            if let Ok(args) = serde_json::from_str(&tool_accum) {
                                                let tc = crate::ToolCall {
                                                    id: id.clone(),
                                                    name: name.clone(),
                                                    arguments: args,
                                                    thought_signature: None,
                                                };
                                                partial
                                                    .content
                                                    .push(crate::Content::ToolCall(tc.clone()));
                                                let _ = tx
                                                    .send(AssistantMessageEvent::ToolCallEnd {
                                                        content_index,
                                                        tool_call: tc,
                                                        partial: partial.clone(),
                                                    })
                                                    .await;
                                            }
                                        }
                                        Some(BlockType::Thinking)
                                        | Some(BlockType::RedactedThinking) => {
                                            let thinking = std::mem::take(&mut thinking_accum);
                                            let sig = std::mem::take(&mut thinking_signature);
                                            let redacted = matches!(
                                                &current_block,
                                                Some(BlockType::RedactedThinking)
                                            );
                                            partial.content.push(crate::Content::Thinking {
                                                thinking: thinking.clone(),
                                                thinking_signature: sig,
                                                redacted,
                                            });
                                            let _ = tx
                                                .send(AssistantMessageEvent::ThinkingEnd {
                                                    content_index,
                                                    thinking,
                                                    partial: partial.clone(),
                                                })
                                                .await;
                                        }
                                        None => {}
                                    }
                                    current_block = None;
                                    content_index += 1;
                                }
                                "message_delta" => {
                                    if let Some(delta) = event["delta"].as_object()
                                        && let Some(sr) =
                                            delta.get("stop_reason").and_then(|v| v.as_str())
                                    {
                                        partial.stop_reason = match sr {
                                            "end_turn" => crate::StopReason::Stop,
                                            "max_tokens" => crate::StopReason::Length,
                                            "tool_use" => crate::StopReason::ToolUse,
                                            "refusal" => crate::StopReason::Error,
                                            _ => crate::StopReason::Stop,
                                        };
                                    }
                                    if let Some(u) = event["usage"].as_object() {
                                        partial.usage.output_tokens = u["output_tokens"]
                                            .as_u64()
                                            .unwrap_or(partial.usage.output_tokens);
                                        partial.usage.total_tokens = partial.usage.compute_total();
                                    }
                                }
                                "message_stop" => {
                                    let _ = tx
                                        .send(AssistantMessageEvent::Done {
                                            reason: partial.stop_reason.clone(),
                                            message: partial.clone(),
                                        })
                                        .await;
                                    return Ok(());
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Err(e) => {
                    return Err(LlmError::StreamError(format!("SSE stream error: {e}")));
                }
            }
        }

        Ok(())
    }
}

fn build_cache_control(retention: crate::cache::CacheRetention) -> Option<serde_json::Value> {
    match retention {
        crate::cache::CacheRetention::None => None,
        crate::cache::CacheRetention::Short => Some(serde_json::json!({"type": "ephemeral"})),
        crate::cache::CacheRetention::Long => {
            Some(serde_json::json!({"type": "ephemeral", "ttl": "1h"}))
        }
    }
}

fn is_adaptive_model(model_id: &str) -> bool {
    model_id.contains("opus-4-6")
        || model_id.contains("opus-4.6")
        || model_id.contains("opus-4-7")
        || model_id.contains("opus-4.7")
        || model_id.contains("sonnet-4-6")
        || model_id.contains("sonnet-4.6")
        || model_id.contains("haiku-4-7")
}

fn map_effort(level: crate::provider::ReasoningLevel, model_id: &str) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_name() {
        let p = AnthropicProvider::new(None);
        assert_eq!(p.provider_name(), "anthropic");
    }

    #[test]
    fn test_models() {
        let p = AnthropicProvider::new(None);
        let m = p.models();
        assert!(m.contains(&"claude-sonnet-4-20250514".to_string()));
        assert!(m.contains(&"claude-opus-4-7".to_string()));
        assert!(m.len() >= 3);
    }

    #[test]
    fn test_is_adaptive_true() {
        assert!(super::is_adaptive_model("claude-opus-4-7"));
        assert!(super::is_adaptive_model("claude-opus-4.7"));
        assert!(super::is_adaptive_model("claude-sonnet-4-6"));
        assert!(super::is_adaptive_model("claude-haiku-4-7"));
    }

    #[test]
    fn test_is_adaptive_false() {
        assert!(!super::is_adaptive_model("claude-sonnet-4-20250514"));
        assert!(!super::is_adaptive_model("claude-opus-3"));
    }

    #[test]
    fn test_map_effort() {
        use crate::provider::ReasoningLevel;
        assert_eq!(
            super::map_effort(ReasoningLevel::Minimal, "any-model"),
            "low"
        );
        assert_eq!(super::map_effort(ReasoningLevel::High, "any-model"), "high");
        assert_eq!(
            super::map_effort(ReasoningLevel::XHigh, "claude-opus-4.7"),
            "xhigh"
        );
        assert_eq!(
            super::map_effort(ReasoningLevel::XHigh, "claude-haiku-4-7"),
            "high"
        );
    }
}
