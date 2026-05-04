use secrecy::{ExposeSecret, SecretString};
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
#[cfg(test)]
use crate::provider::LlmProvider;
use crate::streaming::AssistantMessageEvent;
use crate::types::{Api, LlmContext};

crate::providers::shared::define_provider!(
    MistralProvider,
    "mistral",
    "MISTRAL_API_KEY",
    "https://api.mistral.ai/v1/chat/completions",
    "mistral-large-latest",
    "mistral-medium-latest"
);

impl MistralProvider {
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
        // Build messages array (OpenAI-compatible format)
        let mut messages: Vec<serde_json::Value> = Vec::new();

        if let Some(sp) = &context.system_prompt {
            messages.push(serde_json::json!({
                "role": "system",
                "content": sp,
            }));
        }

        for msg in &context.messages {
            messages.push(match msg {
                crate::Message::User(m) => serde_json::json!({
                    "role": "user",
                    "content": m.content.iter().map(|c| match c {
                        crate::Content::Text { text, .. } => serde_json::json!({"type": "text", "text": text}),
                        crate::Content::Image { data, mime_type } => serde_json::json!({"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", mime_type, data)}}),
                        _ => serde_json::json!({"type": "text", "text": ""}),
                    }).collect::<Vec<_>>(),
                }),
                crate::Message::Assistant(m) => serde_json::json!({
                    "role": "assistant",
                    "content": m.content.iter().map(|c| match c {
                        crate::Content::Text { text, .. } => serde_json::json!({"type": "text", "text": text}),
                        crate::Content::ToolCall(tc) => serde_json::json!({"type": "function", "id": truncate_tool_call_id(&tc.id), "name": tc.name, "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default()}),
                        _ => serde_json::json!({"type": "text", "text": ""}),
                    }).collect::<Vec<_>>(),
                }),
                crate::Message::ToolResult(m) => serde_json::json!({
                    "role": "tool",
                    "tool_call_id": truncate_tool_call_id(&m.tool_call_id),
                    "content": m.content.iter().filter_map(|c| match c {
                        crate::Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    }).collect::<Vec<_>>().join("\n"),
                }),
            });
        }

        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "max_completion_tokens": options.max_tokens.unwrap_or(4096),
        });

        if let Some(tools) = &context.tools {
            body["tools"] = serde_json::json!(
                tools
                    .iter()
                    .map(|t| serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        },
                    }))
                    .collect::<Vec<_>>()
            );
        }

        if let Some(temp) = options.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        // Reasoning: Mistral-specific format
        if let Some(level) = options.reasoning {
            body["promptMode"] = serde_json::json!("reasoning");
            let effort = match level {
                crate::provider::ReasoningLevel::Minimal | crate::provider::ReasoningLevel::Low => "low",
                crate::provider::ReasoningLevel::Medium => "medium",
                crate::provider::ReasoningLevel::High | crate::provider::ReasoningLevel::XHigh => "high",
            };
            body["reasoningEffort"] = serde_json::json!(effort);
        }

        // on_payload hook
        if let Some(hook) = &options.on_payload {
            let placeholder = crate::Model {
                id: model.to_string(),
                name: model.to_string(),
                api: "openai-completions".to_string(),
                provider: "mistral".to_string(),
                base_url: base_url.clone(),
                reasoning: true,
                input_modalities: vec![],
                cost: Default::default(),
                context_window: 128_000,
                max_tokens: 128_000,
                headers: None,
                compat: crate::models::ModelCompat::None,
            };
            hook(&mut body, &placeholder).await;
        }

        // Send request
        let response = client
            .post(&base_url)
            .header(
                "Authorization",
                format!("Bearer {}", api_key.expose_secret()),
            )
            .header("content-type", "application/json")
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

        if let Some(hook) = &options.on_response {
            let placeholder = crate::Model {
                id: model.to_string(),
                name: model.to_string(),
                api: "openai-completions".to_string(),
                provider: "mistral".to_string(),
                base_url: base_url.clone(),
                reasoning: true,
                input_modalities: vec![],
                cost: Default::default(),
                context_window: 128_000,
                max_tokens: 128_000,
                headers: None,
                compat: crate::models::ModelCompat::None,
            };
            hook(&crate::ProviderResponse { status, headers }, &placeholder).await;
        }

        if !status.to_string().starts_with('2') {
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::ProviderError(format!("HTTP {status}: {body}")));
        }

        // Process SSE stream (OpenAI-compatible format)
        use futures::StreamExt;
        let mut sse_stream = response.bytes_stream();
        let mut partial = crate::AssistantMessage {
            content: vec![],
            provider: "mistral".to_string(),
            model: model.to_string(),
            api: Api {
                provider: "mistral".to_string(),
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

        let text_content_index: usize = 0;
        let mut text_accum: String = String::new();
        let mut text_started = false;
        let mut tool_call_accum: std::collections::BTreeMap<
            usize,
            (Option<String>, Option<String>, String),
        > = std::collections::BTreeMap::new();

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
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                if !text_accum.is_empty() {
                                    let _ = tx
                                        .send(AssistantMessageEvent::TextEnd {
                                            content_index: text_content_index,
                                            text: std::mem::take(&mut text_accum),
                                            partial: partial.clone(),
                                        })
                                        .await;
                                }
                                for (ci, (id, name, args)) in &tool_call_accum {
                                    if let Ok(args) = serde_json::from_str(args) {
                                        let _ = tx
                                            .send(AssistantMessageEvent::ToolCallEnd {
                                                content_index: *ci,
                                                tool_call: crate::ToolCall {
                                                    id: id
                                                        .clone()
                                                        .unwrap_or_else(|| format!("call_{}", ci)),
                                                    name: name.clone().unwrap_or_default(),
                                                    arguments: args,
                                                    thought_signature: None,
                                                },
                                                partial: partial.clone(),
                                            })
                                            .await;
                                    }
                                }
                                let _ = tx
                                    .send(AssistantMessageEvent::Done {
                                        reason: partial.stop_reason.clone(),
                                        message: partial.clone(),
                                    })
                                    .await;
                                return Ok(());
                            }
                            if let Ok(chunk) = serde_json::from_str::<serde_json::Value>(data)
                                && let Some(choices) = chunk["choices"].as_array()
                            {
                                for choice in choices {
                                    let delta = &choice["delta"];

                                    // Text content
                                    if let Some(text) = delta["content"].as_str() {
                                        if !text_started {
                                            text_started = true;
                                            let _ = tx
                                                .send(AssistantMessageEvent::TextStart {
                                                    content_index: text_content_index,
                                                    partial: partial.clone(),
                                                })
                                                .await;
                                        }
                                        text_accum.push_str(text);
                                        let _ = tx
                                            .send(AssistantMessageEvent::TextDelta {
                                                content_index: text_content_index,
                                                delta: text.to_string(),
                                                partial: partial.clone(),
                                            })
                                            .await;
                                    }

                                    // Tool calls
                                    if let Some(tool_calls) = delta["tool_calls"].as_array() {
                                        for tc in tool_calls {
                                            let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                                            let entry = tool_call_accum.entry(idx).or_default();
                                            if let Some(id) = tc["id"].as_str() {
                                                entry.0 = Some(truncate_tool_call_id(id));
                                            }
                                            if let Some(name) = tc["function"]["name"].as_str() {
                                                entry.1 = Some(name.to_string());
                                            }
                                            if let Some(args) = tc["function"]["arguments"].as_str()
                                            {
                                                entry.2.push_str(args);
                                                let _ = tx
                                                    .send(AssistantMessageEvent::ToolCallDelta {
                                                        content_index: idx + 1,
                                                        delta: args.to_string(),
                                                        partial: partial.clone(),
                                                    })
                                                    .await;
                                            }
                                        }
                                    }

                                    // Finish reason
                                    if let Some(reason) = choice["finish_reason"].as_str() {
                                        partial.stop_reason = match reason {
                                            "stop" => crate::StopReason::Stop,
                                            "length" => crate::StopReason::Length,
                                            "tool_calls" => crate::StopReason::ToolUse,
                                            "content_filter" => crate::StopReason::Error,
                                            _ => crate::StopReason::Stop,
                                        };
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => return Err(LlmError::StreamError(format!("SSE error: {e}"))),
            }
        }

        Ok(())
    }
}

/// Truncate tool call ID to ≤ 36 characters (Mistral requirement).
fn truncate_tool_call_id(id: &str) -> String {
    if id.len() <= 36 {
        id.to_string()
    } else {
        let hash = crate::transform::short_hash(id);
        format!("call_{}{}", hash, &id[id.len().saturating_sub(8)..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_name() {
        let p = MistralProvider::new(None);
        assert_eq!(p.provider_name(), "mistral");
    }

    #[test]
    fn test_models() {
        let p = MistralProvider::new(None);
        let m = p.models();
        assert!(m.contains(&"mistral-large-latest".to_string()));
        assert!(m.contains(&"mistral-medium-latest".to_string()));
        assert!(m.len() >= 2);
    }

    #[test]
    fn test_truncate_tool_call_id_short() {
        let id = "call_123";
        assert_eq!(truncate_tool_call_id(id), id);
    }

    #[test]
    fn test_truncate_tool_call_id_long() {
        let id = "a".repeat(100);
        let truncated = truncate_tool_call_id(&id);
        assert!(truncated.len() <= 36);
    }
}
