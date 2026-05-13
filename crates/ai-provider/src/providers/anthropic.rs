use secrecy::{ExposeSecret, SecretString};
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
#[cfg(test)]
use crate::provider::LlmProvider;
use crate::providers::anthropic_common as common;
use crate::streaming::AssistantMessageEvent;
use crate::types::LlmContext;

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
        // Build request body
        let mut messages_json = common::build_messages_json(&context.messages);
        common::apply_cache_to_last_user_message(&mut messages_json, options.cache_retention);

        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": options.max_tokens.unwrap_or(4096),
            "messages": messages_json,
        });

        // System prompt
        if let Some(system_prompt) = &context.system_prompt {
            body["system"] = serde_json::json!(common::build_system_blocks(
                system_prompt,
                options.cache_retention,
            ));
        }

        // Tools
        if let Some(tools) = &context.tools {
            body["tools"] =
                serde_json::json!(common::build_tools_json(tools, options.cache_retention));
        }

        // Thinking / reasoning
        let max_tokens = options.max_tokens.unwrap_or(4096);
        let (new_max, thinking_config) = common::build_thinking_config(
            options.reasoning,
            model,
            max_tokens,
            options.thinking_budgets.as_ref(),
        );
        body["max_tokens"] = serde_json::json!(new_max);
        match thinking_config {
            common::ThinkingConfig::Disabled => {
                body["thinking"] = serde_json::json!({"type": "disabled"});
            }
            common::ThinkingConfig::Adaptive { effort } => {
                body["thinking"] = serde_json::json!({"type": "adaptive", "display": "summarized"});
                body["output_config"] = serde_json::json!({"effort": effort});
            }
            common::ThinkingConfig::Enabled { budget_tokens } => {
                body["thinking"] = serde_json::json!({"type": "enabled", "budget_tokens": budget_tokens, "display": "summarized"});
            }
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

        if status < 200 || status >= 300 {
            let body = response.text().await.map_err(|e| {
                LlmError::ProviderError(format!("failed to read response body: {e}"))
            })?;
            tracing::error!(
                status = %status,
                body = %body,
                provider = "anthropic",
                "HTTP error response from provider"
            );
            let msg = crate::http_error::sanitize_http_error_body(status, &body);
            return Err(LlmError::ProviderError(msg));
        }

        // Process SSE stream with full event mapping per spec §9.1
        use futures::StreamExt;
        let mut sse_stream = response.bytes_stream();

        let mut parser = common::StreamParser::new("anthropic", model);
        let _ = tx
            .send(AssistantMessageEvent::Start {
                partial: parser.partial.clone(),
            })
            .await;

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
                            && let Ok(Some(_)) = parser.process_event(&event, tx).await
                        {
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    return Err(LlmError::StreamError {
                        kind: crate::StreamErrorKind::Network,
                        message: format!("SSE stream error: {e}"),
                    });
                }
            }
        }

        Ok(())
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
        assert!(crate::providers::anthropic_common::is_adaptive_model(
            "claude-opus-4-7"
        ));
        assert!(crate::providers::anthropic_common::is_adaptive_model(
            "claude-opus-4.7"
        ));
        assert!(crate::providers::anthropic_common::is_adaptive_model(
            "claude-sonnet-4-6"
        ));
        assert!(crate::providers::anthropic_common::is_adaptive_model(
            "claude-haiku-4-7"
        ));
    }

    #[test]
    fn test_is_adaptive_false() {
        assert!(!crate::providers::anthropic_common::is_adaptive_model(
            "claude-sonnet-4-20250514"
        ));
        assert!(!crate::providers::anthropic_common::is_adaptive_model(
            "claude-opus-3"
        ));
    }

    #[test]
    fn test_map_effort() {
        use crate::provider::ReasoningLevel;
        assert_eq!(
            crate::providers::anthropic_common::map_effort(ReasoningLevel::Minimal, "any-model"),
            "low"
        );
        assert_eq!(
            crate::providers::anthropic_common::map_effort(ReasoningLevel::High, "any-model"),
            "high"
        );
        assert_eq!(
            crate::providers::anthropic_common::map_effort(
                ReasoningLevel::XHigh,
                "claude-opus-4.7"
            ),
            "xhigh"
        );
        assert_eq!(
            crate::providers::anthropic_common::map_effort(
                ReasoningLevel::XHigh,
                "claude-haiku-4-7"
            ),
            "high"
        );
    }
}
