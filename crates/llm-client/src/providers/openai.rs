use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::oauth::resolve_oauth_key;
use crate::provider::LlmProvider;
use crate::streaming::{AssistantMessageEvent, AssistantMessageEventStream};
use crate::types::{Api, LlmContext};

pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: Option<SecretString>,
    base_url: String,
    oauth_provider: Option<std::sync::Arc<dyn crate::oauth::OAuthProvider>>,
}

impl OpenAiProvider {
    pub fn new(api_key: Option<SecretString>) -> Self {
        Self::with_base_url(api_key, "https://api.openai.com/v1/chat/completions")
    }

    pub fn with_base_url(api_key: Option<SecretString>, base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("reqwest client should build");
        Self {
            client,
            api_key,
            base_url: base_url.to_string(),
            oauth_provider: None,
        }
    }

    pub fn with_oauth(
        mut self,
        oauth: std::sync::Arc<dyn crate::oauth::OAuthProvider>,
    ) -> Self {
        self.oauth_provider = Some(oauth);
        self
    }

    fn resolve_api_key(
        &self,
        options: &crate::provider::StreamOptions,
    ) -> Result<SecretString, LlmError> {
        if let Some(key) = &options.api_key {
            return Ok(key.clone());
        }
        if let Some(key) = &self.api_key {
            return Ok(key.clone());
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            return Ok(SecretString::new(key.into_boxed_str()));
        }
        Err(LlmError::AuthError("OPENAI_API_KEY not set".to_string()))
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn provider_name(&self) -> &str {
        "openai"
    }

    fn models(&self) -> Vec<String> {
        vec![
            "gpt-5.2".to_string(),
            "gpt-5.1".to_string(),
            "gpt-4.1".to_string(),
        ]
    }

    async fn stream(
        &self,
        model: &str,
        context: LlmContext,
        options: crate::provider::StreamOptions,
        signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError> {
        // Try OAuth first, fall back to static key on any failure
        let api_key = if let Some(key) = resolve_oauth_key(&self.oauth_provider).await {
            key
        } else {
            self.resolve_api_key(&options)?
        };
        let (stream, tx) = AssistantMessageEventStream::new(32);
        let client = self.client.clone();
        let model = model.to_string();

        let base_url = self.base_url.clone();
        tokio::spawn(async move {
            let result =
                Self::try_stream(client, base_url, &model, context, options, &tx, api_key, signal).await;
            if let Err(e) = result {
                let _ = tx
                    .send(AssistantMessageEvent::Error {
                        error: crate::AssistantMessage {
                            content: vec![],
                            provider: "openai".to_string(),
                            model: model.clone(),
                            api: Api {
                                provider: "openai".to_string(),
                                model,
                            },
                            usage: crate::Usage {
                                input_tokens: 0,
                                output_tokens: 0,
                                cache_creation_input_tokens: None,
                                cache_read_input_tokens: None,
                                total_tokens: 0,
                            },
                            stop_reason: crate::StopReason::Error,
                            response_id: None,
                            error_message: Some(e.to_string()),
                            timestamp: std::time::SystemTime::now(),
                        },
                    })
                    .await;
            }
        });

        Ok(stream)
    }
}

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
                crate::Message::Assistant(m) => serde_json::json!({
                    "role": "assistant",
                    "content": m.content.iter().map(|c| match c {
                        crate::Content::Text { text, .. } => serde_json::json!({"type": "text", "text": text}),
                        crate::Content::ToolCall(tc) => serde_json::json!({"type": "function", "id": tc.id, "name": tc.name, "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default()}),
                        _ => serde_json::json!({"type": "text", "text": ""}),
                    }).collect::<Vec<_>>(),
                }),
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
        let effective_reasoning = if options.reasoning == Some(crate::provider::ReasoningLevel::XHigh)
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
                        map_reasoning_effort(level)
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

        // on_payload hook
        if let Some(hook) = &options.on_payload {
            let placeholder = crate::Model {
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
            };
            hook(&mut body, &placeholder).await;
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
            let placeholder = crate::Model {
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
            };
            hook(&crate::ProviderResponse { status, headers }, &placeholder).await;
        }

        if !status.to_string().starts_with('2') {
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::ProviderError(format!("HTTP {status}: {body}")));
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
    fn test_models_not_empty() {
        let p = OpenAiProvider::new(None);
        assert!(!p.models().is_empty());
    }
}
