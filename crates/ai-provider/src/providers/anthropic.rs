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
    async fn try_stream_inner(
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

        let fallback = crate::protocol::request::fallback_model(
            "anthropic",
            model,
            "anthropic-messages",
            &base_url,
            200_000,
            8192,
        );

        let response =
            crate::protocol::request::RequestBuilder::new(client, base_url, fallback, options)
                .body(body)
                .header("x-api-key", api_key.expose_secret())
                .header("anthropic-version", "2023-06-01")
                .header(
                    "anthropic-beta",
                    "interleaved-thinking-2025-05-14, fine-grained-tool-streaming-2025-05-14",
                )
                .send()
                .await?;

        // Process SSE stream with full event mapping per spec §9.1
        use futures::StreamExt;
        let sse_stream = response.bytes_stream();
        let mut decoder = crate::protocol::sse::SseDecoder::new(sse_stream, signal);

        let mut parser = common::StreamParser::new("anthropic", model);
        let _ = tx
            .send(AssistantMessageEvent::Start {
                partial: parser.partial.clone(),
            })
            .await;

        while let Some(result) = decoder.next().await {
            let event = result?;
            if event.data.trim().is_empty() {
                continue;
            }
            let json_event: serde_json::Value = event.json()?;
            if let Ok(Some(_)) = parser.process_event(&json_event, tx).await {
                return Ok(());
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
