use secrecy::SecretString;
use std::sync::Arc;

use crate::error::LlmError;
use crate::oauth::OAuthProvider;
use crate::provider::StreamOptions;

/// Shared configuration and state for all HTTP-based LLM providers.
pub struct ProviderConfig {
    pub client: reqwest::Client,
    pub api_key: Option<SecretString>,
    pub base_url: String,
    pub oauth_provider: Option<Arc<dyn OAuthProvider>>,
    pub provider_name: String,
    pub env_key: &'static str,
}

impl ProviderConfig {
    pub fn new(
        api_key: Option<SecretString>,
        base_url: &str,
        provider_name: &str,
        env_key: &'static str,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("reqwest client should build");
        Self {
            client,
            api_key,
            base_url: base_url.to_string(),
            oauth_provider: None,
            provider_name: provider_name.to_string(),
            env_key,
        }
    }

    pub fn with_client(
        client: reqwest::Client,
        api_key: Option<SecretString>,
        base_url: &str,
        provider_name: &str,
        env_key: &'static str,
    ) -> Self {
        Self {
            client,
            api_key,
            base_url: base_url.to_string(),
            oauth_provider: None,
            provider_name: provider_name.to_string(),
            env_key,
        }
    }

    pub fn with_oauth(mut self, oauth: Arc<dyn OAuthProvider>) -> Self {
        self.oauth_provider = Some(oauth);
        self
    }

    pub fn resolve_api_key(&self, options: &StreamOptions) -> Result<SecretString, LlmError> {
        if let Some(key) = &options.api_key {
            return Ok(key.clone());
        }
        if let Some(key) = &self.api_key {
            return Ok(key.clone());
        }
        if let Ok(key) = std::env::var(self.env_key) {
            return Ok(SecretString::new(key.into_boxed_str()));
        }
        Err(LlmError::AuthError(
            format!("{} not set", self.env_key),
        ))
    }
}

macro_rules! define_provider {
    ($struct_name:ident, $provider_str:literal, $env_key:literal, $default_url:literal) => {
        #[doc = concat!("LLM provider for the ", $provider_str, " API.")]
        #[doc = ""]
        #[doc = concat!("Implements the `LlmProvider` trait for ", $provider_str, " models.")]
        #[doc = "Requires the corresponding API key environment variable or explicit key."]
        pub struct $struct_name {
            config: crate::providers::shared::ProviderConfig,
        }

        impl $struct_name {
            /// Create a new provider with the default API endpoint.
            pub fn new(api_key: Option<secrecy::SecretString>) -> Self {
                Self::with_base_url(api_key, $default_url)
            }

            /// Create a new provider with a custom base URL.
            pub fn with_base_url(
                api_key: Option<secrecy::SecretString>,
                base_url: &str,
            ) -> Self {
                Self {
                    config: crate::providers::shared::ProviderConfig::new(
                        api_key,
                        base_url,
                        $provider_str,
                        $env_key,
                    ),
                }
            }

            /// Create a new provider with an externally-managed HTTP client.
            pub fn with_client(
                client: reqwest::Client,
                api_key: Option<secrecy::SecretString>,
                base_url: &str,
            ) -> Self {
                Self {
                    config: crate::providers::shared::ProviderConfig::with_client(
                        client,
                        api_key,
                        base_url,
                        $provider_str,
                        $env_key,
                    ),
                }
            }

            /// Attach an OAuth provider for automatic token management.
            pub fn with_oauth(
                mut self,
                oauth: std::sync::Arc<dyn crate::oauth::OAuthProvider>,
            ) -> Self {
                self.config = self.config.with_oauth(oauth);
                self
            }
        }

        #[async_trait::async_trait]
        impl crate::provider::LlmProvider for $struct_name {
            fn provider_name(&self) -> &str {
                &self.config.provider_name
            }

            fn models(&self) -> Vec<String> {
                crate::models::models_for_provider_names($provider_str)
            }

            fn config(&self) -> &crate::providers::shared::ProviderConfig {
                &self.config
            }

            async fn stream(
                &self,
                model: &str,
                context: crate::types::LlmContext,
                options: crate::provider::StreamOptions,
                signal: tokio_util::sync::CancellationToken,
            ) -> Result<crate::streaming::AssistantMessageEventStream, crate::error::LlmError> {
                let config = self.config();
                let api_key =
                    if let Some(key) = crate::oauth::resolve_oauth_key(config.oauth_provider.as_ref()).await
                    {
                        key
                    } else {
                        config.resolve_api_key(&options)?
                    };
                let (stream, tx) = crate::streaming::AssistantMessageEventStream::new(32);
                let client = config.client.clone();
                let model = model.to_string();
                let base_url = config.base_url.clone();
                let provider_name = self.provider_name().to_string();
                let provider_name_clone = provider_name.clone();

                let handle = tokio::spawn(async move {
                    let result = Self::try_stream_inner(
                        client,
                        base_url,
                        &model,
                        context,
                        options,
                        &tx,
                        api_key,
                        signal,
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
    };
}

pub(crate) use define_provider;
