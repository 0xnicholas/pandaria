use secrecy::{ExposeSecret, SecretString};
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
#[cfg(test)]
use crate::provider::LlmProvider;
use crate::streaming::AssistantMessageEvent;
use crate::types::{Api, LlmContext};

crate::providers::shared::define_provider!(
    GoogleProvider,
    "google",
    "GOOGLE_API_KEY",
    "https://generativelanguage.googleapis.com/v1beta"
);

impl GoogleProvider {
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
        // Build contents (spec §9.3)
        let mut contents: Vec<serde_json::Value> = Vec::new();

        for msg in &context.messages {
            contents.push(match msg {
                crate::Message::User(m) => serde_json::json!({
                    "role": "user",
                    "parts": m.content.iter().map(|c| match c {
                        crate::Content::Text { text, .. } => serde_json::json!({"text": text}),
                        _ => serde_json::json!({"text": ""}),
                    }).collect::<Vec<_>>(),
                }),
                crate::Message::Assistant(m) => serde_json::json!({
                    "role": "model",
                    "parts": m.content.iter().map(|c| match c {
                        crate::Content::Text { text, .. } => serde_json::json!({"text": text}),
                        crate::Content::ToolCall(tc) => serde_json::json!({
                            "functionCall": { "name": tc.name, "args": tc.arguments }
                        }),
                        crate::Content::Thinking { thinking, thinking_signature, .. } => {
                            let mut part = serde_json::json!({"text": thinking, "thought": true});
                            if let Some(sig) = thinking_signature {
                                part["thoughtSignature"] = serde_json::json!(sig);
                            }
                            part
                        },
                        _ => serde_json::json!({"text": ""}),
                    }).collect::<Vec<_>>(),
                }),
                crate::Message::ToolResult(m) => serde_json::json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": m.tool_name,
                            "response": m.content.iter().filter_map(|c| match c {
                                crate::Content::Text { text, .. } => Some(serde_json::json!({"text": text})),
                                _ => None,
                            }).collect::<Vec<_>>(),
                        }
                    }],
                }),
            });
        }

        let mut body = serde_json::json!({
            "contents": contents,
            "generationConfig": {
                "maxOutputTokens": options.max_tokens.unwrap_or(4096),
            },
        });

        if let Some(sp) = &context.system_prompt {
            body["systemInstruction"] = serde_json::json!({
                "parts": [{"text": sp}]
            });
        }

        if let Some(tools) = &context.tools {
            body["tools"] = serde_json::json!([{
                "functionDeclarations": tools.iter().map(|t| serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "parametersJsonSchema": t.parameters,
                })).collect::<Vec<_>>(),
            }]);
        }

        if let Some(temp) = options.temperature {
            body["generationConfig"]["temperature"] = serde_json::json!(temp);
        }

        // on_payload hook
        if let Some(hook) = &options.on_payload {
            let placeholder = crate::Model {
                id: model.to_string(),
                name: model.to_string(),
                api: "google-generative-ai".to_string(),
                provider: "google".to_string(),
                base_url: base_url.clone(),
                reasoning: true,
                input_modalities: vec![],
                cost: Default::default(),
                context_window: 2_097_152,
                max_tokens: 65_535,
                headers: None,
                compat: crate::models::ModelCompat::None,
            };
            hook(&mut body, &placeholder).await;
        }

        // Send request
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            base_url, model
        );

        let mut req = client
            .post(&url)
            .header("x-goog-api-key", api_key.expose_secret())
            .header("content-type", "application/json");

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
            let placeholder = crate::Model {
                id: model.to_string(),
                name: model.to_string(),
                api: "google-generative-ai".to_string(),
                provider: "google".to_string(),
                base_url: base_url.clone(),
                reasoning: true,
                input_modalities: vec![],
                cost: Default::default(),
                context_window: 2_097_152,
                max_tokens: 65_535,
                headers: None,
                compat: crate::models::ModelCompat::None,
            };
            hook(&crate::ProviderResponse { status, headers }, &placeholder).await;
        }

        if status < 200 || status >= 300 {
            let body = response.text().await.map_err(|e| {
                LlmError::ProviderError(format!("failed to read response body: {e}"))
            })?;
            tracing::error!(
                status = %status,
                body = %body,
                provider = "google",
                "HTTP error response from provider"
            );
            let msg = crate::http_error::sanitize_http_error_body(status, &body);
            return Err(LlmError::ProviderError(msg));
        }

        // Process SSE stream (Google Gemini format)
        use futures::StreamExt;
        let mut sse_stream = response.bytes_stream();
        let mut partial = crate::AssistantMessage {
            content: vec![],
            provider: "google".to_string(),
            model: model.to_string(),
            api: Api {
                provider: "google".to_string(),
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

        let mut content_index: usize = 0;
        let mut buffer = String::new();
        let mut current_text_block: bool = false;
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
                            && let Ok(response) = serde_json::from_str::<serde_json::Value>(data)
                            && let Some(candidates) = response["candidates"].as_array()
                        {
                            for candidate in candidates {
                                if let Some(parts) = candidate["content"]["parts"].as_array() {
                                    for part in parts {
                                        let is_thought = part["thought"].as_bool().unwrap_or(false);

                                        if let Some(text) = part["text"].as_str() {
                                            if is_thought {
                                                let _ = tx
                                                    .send(AssistantMessageEvent::ThinkingDelta {
                                                        content_index,
                                                        delta: text.to_string(),
                                                        partial: partial.clone(),
                                                    })
                                                    .await;
                                            } else {
                                                if !current_text_block {
                                                    current_text_block = true;
                                                    let _ = tx
                                                        .send(AssistantMessageEvent::TextStart {
                                                            content_index,
                                                            partial: partial.clone(),
                                                        })
                                                        .await;
                                                }
                                                let _ = tx
                                                    .send(AssistantMessageEvent::TextDelta {
                                                        content_index,
                                                        delta: text.to_string(),
                                                        partial: partial.clone(),
                                                    })
                                                    .await;
                                            }
                                        }

                                        if let Some(fc) = part.get("functionCall") {
                                            if current_text_block {
                                                current_text_block = false;
                                                let _ = tx
                                                    .send(AssistantMessageEvent::TextEnd {
                                                        content_index,
                                                        text: String::new(),
                                                        partial: partial.clone(),
                                                    })
                                                    .await;
                                            }
                                            let tc = crate::ToolCall {
                                                id: format!("call_{}", content_index),
                                                name: fc["name"].as_str().unwrap_or("").to_string(),
                                                arguments: fc["args"].clone(),
                                                thought_signature: None,
                                            };
                                            let _ = tx
                                                .send(AssistantMessageEvent::ToolCallEnd {
                                                    content_index,
                                                    tool_call: tc,
                                                    partial: partial.clone(),
                                                })
                                                .await;
                                            content_index += 1;
                                        }
                                    }
                                }

                                if let Some(reason) = candidate["finishReason"].as_str() {
                                    if current_text_block {
                                        current_text_block = false;
                                        let _ = tx
                                            .send(AssistantMessageEvent::TextEnd {
                                                content_index,
                                                text: String::new(),
                                                partial: partial.clone(),
                                            })
                                            .await;
                                    }
                                    partial.stop_reason = match reason {
                                        "STOP" => crate::StopReason::Stop,
                                        "MAX_TOKENS" => crate::StopReason::Length,
                                        _ => crate::StopReason::Error,
                                    };
                                }
                            }
                        }
                    }
                }
                Err(e) => return Err(LlmError::StreamError {
                    kind: crate::StreamErrorKind::Network,
                    message: format!("SSE error: {e}"),
                }),
            }
        }

        let _ = tx
            .send(AssistantMessageEvent::Done {
                reason: partial.stop_reason.clone(),
                message: partial,
            })
            .await;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_name() {
        let p = GoogleProvider::new(None);
        assert_eq!(p.provider_name(), "google");
    }

    #[test]
    fn test_models() {
        let p = GoogleProvider::new(None);
        let m = p.models();
        assert!(m.contains(&"gemini-2.5-pro".to_string()));
        assert!(m.contains(&"gemini-3.0-flash".to_string()));
        assert!(m.len() >= 3);
    }
}
