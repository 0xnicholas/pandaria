macro_rules! define_provider {
    ($struct_name:ident, $provider_str:literal, $env_key:literal, $default_url:literal $(, $model:literal)*) => {
        #[doc = concat!("LLM provider for ", $provider_str)]
        pub struct $struct_name {
            client: reqwest::Client,
            api_key: Option<secrecy::SecretString>,
            base_url: String,
            oauth_provider: Option<std::sync::Arc<dyn crate::oauth::OAuthProvider>>,
        }

        impl $struct_name {
            pub fn new(api_key: Option<secrecy::SecretString>) -> Self {
                Self::with_base_url(api_key, $default_url)
            }

            pub fn with_base_url(
                api_key: Option<secrecy::SecretString>,
                base_url: &str,
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
            ) -> Result<secrecy::SecretString, crate::error::LlmError> {
                if let Some(key) = &options.api_key {
                    return Ok(key.clone());
                }
                if let Some(key) = &self.api_key {
                    return Ok(key.clone());
                }
                if let Ok(key) = std::env::var($env_key) {
                    return Ok(secrecy::SecretString::new(key.into_boxed_str()));
                }
                Err(crate::error::LlmError::AuthError(
                    concat!($env_key, " not set").to_string(),
                ))
            }
        }

        #[async_trait::async_trait]
        impl crate::provider::LlmProvider for $struct_name {
            fn provider_name(&self) -> &str {
                $provider_str
            }

            fn models(&self) -> Vec<String> {
                crate::models::models_for_provider_names($provider_str)
            }

            async fn stream(
                &self,
                model: &str,
                context: crate::types::LlmContext,
                options: crate::provider::StreamOptions,
                signal: tokio_util::sync::CancellationToken,
            ) -> Result<crate::streaming::AssistantMessageEventStream, crate::error::LlmError>
            {
                let api_key =
                    if let Some(key) = crate::oauth::resolve_oauth_key(self.oauth_provider.as_ref()).await
                    {
                        key
                    } else {
                        self.resolve_api_key(&options)?
                    };
                let (stream, tx) = crate::streaming::AssistantMessageEventStream::new(32);
                let client = self.client.clone();
                let model = model.to_string();
                let base_url = self.base_url.clone();

                tokio::spawn(async move {
                    let result = Self::try_stream(
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
                                    provider: $provider_str.to_string(),
                                    model: model.clone(),
                                    api: crate::types::Api {
                                        provider: $provider_str.to_string(),
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
                                        "{} '{}' {}: {}",
                                        $provider_str, model, model, err_msg,
                                    )),
                                    timestamp: std::time::SystemTime::now(),
                                },
                            })
                            .await;
                    }
                });

                Ok(stream)
            }
        }
    };
}

pub(crate) use define_provider;
