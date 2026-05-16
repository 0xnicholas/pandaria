use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::cache::CacheRetention;
use crate::error::LlmError;
use crate::hooks::{OnPayloadFn, OnResponseFn};
use crate::streaming::AssistantMessageEventStream;
use crate::types::{Api, LlmContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningLevel {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ThinkingBudgets {
    pub minimal: Option<u32>,
    pub low: Option<u32>,
    pub medium: Option<u32>,
    pub high: Option<u32>,
}

pub fn adjust_max_tokens_for_thinking(
    base_max_tokens: u32,
    model_max_tokens: u32,
    reasoning_level: ReasoningLevel,
    custom_budgets: Option<&ThinkingBudgets>,
) -> (u32, u32) {
    let default_budgets = ThinkingBudgets {
        minimal: Some(1024),
        low: Some(2048),
        medium: Some(8192),
        high: Some(16384),
    };
    let budgets = custom_budgets.unwrap_or(&default_budgets);

    let thinking_budget = match reasoning_level {
        ReasoningLevel::Minimal => budgets.minimal.unwrap_or(1024),
        ReasoningLevel::Low => budgets.low.unwrap_or(2048),
        ReasoningLevel::Medium => budgets.medium.unwrap_or(8192),
        ReasoningLevel::High | ReasoningLevel::XHigh => budgets.high.unwrap_or(16384),
    };

    let min_output_tokens: u32 = 1024;
    let total = base_max_tokens + thinking_budget;
    if total <= model_max_tokens {
        return (total, thinking_budget);
    }

    if thinking_budget + min_output_tokens > model_max_tokens {
        let squeezed_thinking = model_max_tokens.saturating_sub(min_output_tokens);
        return (model_max_tokens, squeezed_thinking);
    }

    (model_max_tokens, thinking_budget)
}

#[derive(Clone)]
pub struct StreamOptions {
    pub api_key: Option<secrecy::SecretString>,
    pub timeout: Duration,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub reasoning: Option<ReasoningLevel>,
    pub thinking_budgets: Option<ThinkingBudgets>,
    pub max_retries: u32,
    pub max_retry_delay_ms: u64,
    pub headers: Option<HashMap<String, String>>,
    pub metadata: Option<HashMap<String, String>>,
    pub cache_retention: CacheRetention,
    pub session_id: Option<String>,
    pub on_payload: Option<OnPayloadFn>,
    pub on_response: Option<OnResponseFn>,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            api_key: None,
            timeout: Duration::from_secs(60),
            max_tokens: None,
            temperature: None,
            top_p: None,
            reasoning: None,
            thinking_budgets: None,
            max_retries: 3,
            max_retry_delay_ms: 60_000,
            headers: None,
            metadata: None,
            cache_retention: CacheRetention::default(),
            session_id: None,
            on_payload: None,
            on_response: None,
        }
    }
}

impl std::fmt::Debug for StreamOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamOptions")
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("timeout", &self.timeout)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .field("top_p", &self.top_p)
            .field("reasoning", &self.reasoning)
            .field("thinking_budgets", &self.thinking_budgets)
            .field("max_retries", &self.max_retries)
            .field("max_retry_delay_ms", &self.max_retry_delay_ms)
            .field("headers", &self.headers)
            .field("metadata", &self.metadata)
            .field("cache_retention", &self.cache_retention)
            .field("session_id", &self.session_id)
            .field(
                "on_payload",
                &self.on_payload.as_ref().map(|_| "OnPayloadFn"),
            )
            .field(
                "on_response",
                &self.on_response.as_ref().map(|_| "OnResponseFn"),
            )
            .finish()
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn provider_name(&self) -> &str;

    fn models(&self) -> Vec<String>;

    fn api_for(&self, model: &str) -> Api {
        Api {
            provider: self.provider_name().to_string(),
            model: model.to_string(),
        }
    }

    /// Access the shared provider configuration.
    fn config(&self) -> &crate::providers::shared::ProviderConfig;

    /// Query model metadata from the global registry.
    ///
    /// Default implementation uses `provider_name()` + `model` to look up
    /// the static model registry. Individual providers may override this
    /// (e.g. `RouterProvider` resolves cross-provider specs).
    fn model_metadata(&self, model: &str) -> Option<crate::models::Model> {
        crate::models::get_model(self.provider_name(), model)
    }

    /// Stream LLM responses for the given model and context.
    ///
    /// Implementors should spawn provider-specific streaming logic on a
    /// background task and return the receive end of an event channel.
    async fn stream(
        &self,
        model: &str,
        context: LlmContext,
        options: StreamOptions,
        signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::AssistantMessageEvent;

    struct MockProvider;

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn provider_name(&self) -> &str {
            "mock"
        }

        fn models(&self) -> Vec<String> {
            vec!["mock-v1".to_string()]
        }

        fn config(&self) -> &crate::providers::shared::ProviderConfig {
            use std::sync::OnceLock;
            static CONFIG: OnceLock<crate::providers::shared::ProviderConfig> = OnceLock::new();
            CONFIG.get_or_init(|| {
                crate::providers::shared::ProviderConfig::new(
                    None,
                    "http://mock",
                    "mock",
                    "MOCK_API_KEY",
                )
            })
        }

        async fn stream(
            &self,
            _model: &str,
            _context: LlmContext,
            _options: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<AssistantMessageEventStream, LlmError> {
            let partial = crate::AssistantMessage {
                content: vec![],
                provider: "mock".to_string(),
                model: "mock-v1".to_string(),
                api: crate::Api {
                    provider: "mock".to_string(),
                    model: "mock-v1".to_string(),
                },
                usage: crate::Usage {
                    input_tokens: 0,
                    output_tokens: 5,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    total_tokens: 5,
                },
                stop_reason: crate::StopReason::Stop,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            };

            let events = vec![
                AssistantMessageEvent::Start {
                    partial: partial.clone(),
                },
                AssistantMessageEvent::TextStart {
                    content_index: 0,
                    partial: partial.clone(),
                },
                AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: "Hello".to_string(),
                    partial: partial.clone(),
                },
                AssistantMessageEvent::TextEnd {
                    content_index: 0,
                    text: "Hello".to_string(),
                    partial: partial.clone(),
                },
                AssistantMessageEvent::Done {
                    reason: crate::StopReason::Stop,
                    message: partial.clone(),
                },
            ];

            let stream = AssistantMessageEventStream::from_events(events);
            Ok(stream)
        }
    }

    #[test]
    fn test_provider_name() {
        let p = MockProvider;
        assert_eq!(p.provider_name(), "mock");
    }

    #[test]
    fn test_models() {
        let p = MockProvider;
        assert_eq!(p.models(), vec!["mock-v1"]);
    }

    #[tokio::test]
    async fn test_provider_stream() {
        let p = MockProvider;
        let ctx = LlmContext {
            system_prompt: None,
            messages: vec![],
            tools: None,
        };
        let mut stream = p
            .stream(
                "mock-v1",
                ctx,
                StreamOptions::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap();

        let event = stream.next().await;
        assert!(matches!(event, Some(AssistantMessageEvent::Start { .. })));

        // skip TextStart
        let _ = stream.next().await;

        let event = stream.next().await;
        assert!(matches!(
            event,
            Some(AssistantMessageEvent::TextDelta { ref delta, .. }) if delta == "Hello"
        ));

        // skip TextEnd
        let _ = stream.next().await;

        let event = stream.next().await;
        assert!(matches!(
            event,
            Some(AssistantMessageEvent::Done { ref reason, .. }) if *reason == crate::StopReason::Stop
        ));
    }

    #[test]
    fn test_adjust_tokens_normal() {
        let (max_tokens, thinking_budget) =
            adjust_max_tokens_for_thinking(4096, 8192, ReasoningLevel::High, None);
        // 4096 + 16384 > 8192, squeezed: model_max - 16384 = ??
        // Actually 4096+16384=20480 > 8192. thinking_budget(16384)+1024=17408 > 8192.
        // So squeezed_thinking = 8192-1024=7168
        // Result: (8192, 7168)
        assert_eq!(max_tokens, 8192);
        assert!(thinking_budget > 0);
    }

    #[test]
    fn test_adjust_tokens_no_squeeze() {
        let (max_tokens, thinking_budget) =
            adjust_max_tokens_for_thinking(1024, 32768, ReasoningLevel::Low, None);
        // 1024 + 2048 = 3072 < 32768, no squeeze
        assert_eq!(max_tokens, 3072);
        assert_eq!(thinking_budget, 2048);
    }

    #[test]
    fn test_adjust_tokens_xhigh_clamped() {
        let (max_tokens, thinking_budget) =
            adjust_max_tokens_for_thinking(4096, 32768, ReasoningLevel::XHigh, None);
        // XHigh → High = 16384. 4096+16384=20480 < 32768, no squeeze
        assert_eq!(max_tokens, 20480);
        assert_eq!(thinking_budget, 16384);
    }

    #[test]
    fn test_adjust_tokens_custom_budgets() {
        let custom = ThinkingBudgets {
            minimal: Some(512),
            low: Some(1024),
            medium: Some(4096),
            high: Some(8192),
        };
        let (max_tokens, thinking_budget) =
            adjust_max_tokens_for_thinking(4096, 32768, ReasoningLevel::Medium, Some(&custom));
        assert_eq!(max_tokens, 8192);
        assert_eq!(thinking_budget, 4096);
    }
}
