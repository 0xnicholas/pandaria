use secrecy::{ExposeSecret, SecretString};
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
#[cfg(test)]
use crate::provider::LlmProvider;
use crate::streaming::AssistantMessageEvent;
use crate::types::{Api, LlmContext};

crate::providers::shared::define_provider!(
    OpenAiProvider,
    "openai",
    "OPENAI_API_KEY",
    "https://api.openai.com/v1/chat/completions"
);

impl OpenAiProvider {
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
        // Build messages array (spec §9.2)
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
                crate::Message::Assistant(m) => {
                    let mut content = Vec::with_capacity(m.content.len());
                    for c in &m.content {
                        let item = match c {
                            crate::Content::Text { text, .. } => serde_json::json!({"type": "text", "text": text}),
                            crate::Content::ToolCall(tc) => {
                                let args = serde_json::to_string(&tc.arguments)
                                    .map_err(|e| LlmError::Serialization(format!("failed to serialize tool call arguments: {e}")))?;
                                serde_json::json!({"type": "function", "id": tc.id, "name": tc.name, "arguments": args})
                            }
                            _ => serde_json::json!({"type": "text", "text": ""}),
                        };
                        content.push(item);
                    }
                    serde_json::json!({
                        "role": "assistant",
                        "content": content,
                    })
                }
                crate::Message::ToolResult(m) => serde_json::json!({
                    "role": "tool",
                    "tool_call_id": m.tool_call_id,
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
        });

        // Determine max_tokens field name from compat (default: max_completion_tokens)
        let max_tokens_key = match crate::models::get_model("openai", model) {
            Some(m) => match &m.compat {
                crate::models::ModelCompat::OpenAI(c) => match c.max_tokens_field {
                    Some(crate::compat::MaxTokensField::MaxTokens) => "max_tokens",
                    _ => "max_completion_tokens",
                },
                _ => "max_completion_tokens",
            },
            None => "max_completion_tokens",
        };
        body[max_tokens_key] = serde_json::json!(options.max_tokens.unwrap_or(4096));

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

        // Prompt cache key (OpenAI-only)
        if options.cache_retention != crate::cache::CacheRetention::None
            && let Some(sid) = &options.session_id
        {
            body["prompt_cache_key"] = serde_json::json!(sid);
        }
        if options.cache_retention == crate::cache::CacheRetention::Long {
            body["prompt_cache_retention"] = serde_json::json!("24h");
        }

        // Reasoning / thinking_format
        // Step 1: XHigh clamp check
        let effective_reasoning = if options.reasoning
            == Some(crate::provider::ReasoningLevel::XHigh)
            && !crate::models::supports_xhigh(model)
        {
            Some(crate::provider::ReasoningLevel::High)
        } else {
            options.reasoning
        };

        // Step 2: Read model compat thinking_format
        let compat = match crate::models::get_model("openai", model) {
            Some(m) => match &m.compat {
                crate::models::ModelCompat::OpenAI(c) => c.clone(),
                _ => crate::compat::OpenAiCompat::default(),
            },
            None => crate::compat::OpenAiCompat::default(),
        };

        // Step 3: Branch by thinking_format
        if let Some(level) = effective_reasoning {
            match compat.thinking_format {
                Some(crate::compat::ThinkingFormat::OpenAI) | None => {
                    let effort = map_reasoning_effort(level);
                    body["reasoning_effort"] = serde_json::json!(effort);
                }
                Some(crate::compat::ThinkingFormat::OpenRouter) => {
                    let effort = map_reasoning_effort(level);
                    body["reasoning"] = serde_json::json!({ "effort": effort });
                }
                Some(crate::compat::ThinkingFormat::DeepSeek) => {
                    body["thinking"] = serde_json::json!({ "type": "enabled" });
                    let effort = if level == crate::provider::ReasoningLevel::XHigh {
                        "max"
                    } else {
                        "high"
                    };
                    body["reasoning_effort"] = serde_json::json!(effort);
                }
                Some(crate::compat::ThinkingFormat::Zai) => {
                    body["enable_thinking"] = serde_json::json!(true);
                }
                Some(crate::compat::ThinkingFormat::Qwen) => {
                    body["enable_thinking"] = serde_json::json!(true);
                }
                Some(crate::compat::ThinkingFormat::QwenChatTemplate) => {
                    body["chat_template_kwargs"] = serde_json::json!({
                        "enable_thinking": true,
                        "preserve_thinking": true,
                    });
                }
            }
        }

        // on_payload hook — use real model metadata when available
        if let Some(hook) = &options.on_payload {
            let model_meta =
                crate::models::get_model("openai", model).unwrap_or_else(|| crate::Model {
                    id: model.to_string(),
                    name: model.to_string(),
                    api: "openai-completions".to_string(),
                    provider: "openai".to_string(),
                    base_url: base_url.clone(),
                    reasoning: true,
                    input_modalities: vec![],
                    cost: Default::default(),
                    context_window: 272_000,
                    max_tokens: 128_000,
                    headers: None,
                    compat: crate::models::ModelCompat::None,
                });
            hook(&mut body, &model_meta).await;
        }

        // Send request
        let mut req = client
            .post(&base_url)
            .header(
                "Authorization",
                format!("Bearer {}", api_key.expose_secret()),
            )
            .header("content-type", "application/json");

        // Session affinity headers for cache
        if options.cache_retention != crate::cache::CacheRetention::None
            && let Some(sid) = &options.session_id
        {
            req = req.header("x-client-request-id", sid);
        }

        // Merge custom headers from StreamOptions
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

        if let Some(hook) = &options.on_response {
            let model_meta =
                crate::models::get_model("openai", model).unwrap_or_else(|| crate::Model {
                    id: model.to_string(),
                    name: model.to_string(),
                    api: "openai-completions".to_string(),
                    provider: "openai".to_string(),
                    base_url: base_url.clone(),
                    reasoning: true,
                    input_modalities: vec![],
                    cost: Default::default(),
                    context_window: 272_000,
                    max_tokens: 128_000,
                    headers: None,
                    compat: crate::models::ModelCompat::None,
                });
            hook(&crate::ProviderResponse { status, headers }, &model_meta).await;
        }

        if status < 200 || status >= 300 {
            let body = response.text().await.map_err(|e| {
                LlmError::ProviderError(format!("failed to read response body: {e}"))
            })?;
            tracing::error!(
                status = %status,
                body = %body,
                provider = "openai",
                "HTTP error response from provider"
            );
            let msg = crate::http_error::sanitize_http_error_body(status, &body);
            return Err(LlmError::ProviderError(msg));
        }

        // Process SSE stream (OpenAI format)
        use futures::StreamExt;
        let mut sse_stream = response.bytes_stream();
        let mut partial = crate::AssistantMessage {
            content: vec![],
            provider: "openai".to_string(),
            model: model.to_string(),
            api: Api {
                provider: "openai".to_string(),
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
                                // Extract response.id from the first chunk that carries it
                                if partial.response_id.is_none()
                                    && let Some(id) = chunk["id"].as_str()
                                {
                                    partial.response_id = Some(id.to_string());
                                }

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
                                                entry.0 = Some(id.to_string());
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

                                    // Reasoning/thinking
                                    let reasoning = delta["reasoning_content"]
                                        .as_str()
                                        .or_else(|| delta["reasoning"].as_str())
                                        .or_else(|| delta["reasoning_text"].as_str());
                                    if let Some(r) = reasoning {
                                        let _ = tx
                                            .send(AssistantMessageEvent::ThinkingDelta {
                                                content_index: 0,
                                                delta: r.to_string(),
                                                partial: partial.clone(),
                                            })
                                            .await;
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

fn map_reasoning_effort(level: crate::provider::ReasoningLevel) -> &'static str {
    match level {
        crate::provider::ReasoningLevel::Minimal => "minimal",
        crate::provider::ReasoningLevel::Low => "low",
        crate::provider::ReasoningLevel::Medium => "medium",
        crate::provider::ReasoningLevel::High => "high",
        crate::provider::ReasoningLevel::XHigh => "high",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_name() {
        let p = OpenAiProvider::new(None);
        assert_eq!(p.provider_name(), "openai");
    }

    #[test]
    fn test_models() {
        let p = OpenAiProvider::new(None);
        let m = p.models();
        assert!(m.contains(&"gpt-5.2".to_string()));
        assert!(m.contains(&"gpt-4.1".to_string()));
        assert!(m.len() >= 3);
    }
}
