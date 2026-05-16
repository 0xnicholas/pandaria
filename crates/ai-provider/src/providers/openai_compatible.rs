use async_trait::async_trait;
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::provider::{LlmProvider, StreamOptions};
use crate::providers::shared::ProviderConfig;
use crate::streaming::AssistantMessageEventStream;
use crate::types::LlmContext;

/// 支持运行时 provider_name 覆盖的 OpenAI-compatible provider。
///
/// 用于 OpenRouter、Ollama 等需要自定义 provider_name 的场景，
/// 确保 `detect_openai_compat`、`get_model` 和事件 metadata 中使用正确的名称。
pub struct OpenAiCompatibleProvider {
    config: ProviderConfig,
    override_name: String,
}

impl OpenAiCompatibleProvider {
    /// 创建新的 OpenAI-compatible provider。
    pub fn new(
        api_key: Option<SecretString>,
        base_url: &str,
        provider_name: &str,
        env_key: &'static str,
    ) -> Self {
        Self {
            config: ProviderConfig::new(api_key, base_url, provider_name, env_key),
            override_name: provider_name.to_string(),
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn provider_name(&self) -> &str {
        &self.override_name
    }

    fn models(&self) -> Vec<String> {
        // RouterProvider 负责聚合，底层返回空列表
        vec![]
    }

    fn config(&self) -> &ProviderConfig {
        &self.config
    }

    async fn stream(
        &self,
        model: &str,
        context: LlmContext,
        options: StreamOptions,
        signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError> {
        let config = &self.config;
        let api_key = if let Some(key) =
            crate::oauth::resolve_oauth_key(config.oauth_provider.as_ref()).await
        {
            key
        } else {
            config.resolve_api_key(&options)?
        };
        let (stream, tx) = AssistantMessageEventStream::new(32);
        let client = config.client.clone();
        let model = model.to_string();
        let base_url = config.base_url.clone();
        let provider_name = self.override_name.clone();
        let provider_name_clone = provider_name.clone();

        let handle = tokio::spawn(async move {
            let result = crate::providers::openai::openai_compatible_stream(
                client,
                base_url,
                &model,
                context,
                options,
                &tx,
                api_key,
                signal,
                &provider_name_clone,
            )
            .await;
            if let Err(e) = result {
                let err_msg = e.to_string();
                let _ = tx
                    .send(crate::streaming::AssistantMessageEvent::Error {
                        error: crate::AssistantMessage {
                            content: vec![],
                            provider: provider_name_clone.clone(),
                            model: model.clone(),
                            api: crate::types::Api {
                                provider: provider_name_clone.clone(),
                                model: model.clone(),
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
                            error_message: Some(format!(
                                "{} '{}': {}",
                                provider_name_clone, model, err_msg,
                            )),
                            timestamp: std::time::SystemTime::now(),
                        },
                    })
                    .await;
            }
        });
        tokio::spawn(async move {
            if let Err(e) = handle.await {
                tracing::error!(
                    provider = %provider_name,
                    error = %e,
                    "LLM provider task panicked"
                );
            }
        });

        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_name_override() {
        let p = OpenAiCompatibleProvider::new(
            None,
            "https://openrouter.ai/api/v1/chat/completions",
            "openrouter",
            "OPENROUTER_API_KEY",
        );
        assert_eq!(p.provider_name(), "openrouter");
    }

    #[test]
    fn test_provider_name_ollama() {
        let p = OpenAiCompatibleProvider::new(
            None,
            "http://localhost:11434/v1/chat/completions",
            "ollama",
            "OLLAMA_API_KEY",
        );
        assert_eq!(p.provider_name(), "ollama");
    }

    #[test]
    fn test_models_empty() {
        let p = OpenAiCompatibleProvider::new(
            None,
            "https://example.com/v1",
            "custom",
            "CUSTOM_API_KEY",
        );
        assert!(p.models().is_empty());
    }

    #[test]
    fn test_config_returns_internal() {
        let p = OpenAiCompatibleProvider::new(
            None,
            "https://example.com/v1",
            "custom",
            "CUSTOM_API_KEY",
        );
        assert_eq!(p.config().provider_name, "custom");
        assert_eq!(p.config().env_key, "CUSTOM_API_KEY");
    }
}
