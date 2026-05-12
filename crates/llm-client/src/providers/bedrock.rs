use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::provider::LlmProvider;
use crate::streaming::AssistantMessageEventStream;
use crate::types::LlmContext;

/// AWS Bedrock provider for Claude models.
///
/// Uses `aws-sdk-bedrockruntime::Client` to call `invoke_model_with_response_stream`.
/// The request/response format follows the Anthropic Messages API.
pub struct AwsBedrockProvider {
    client: aws_sdk_bedrockruntime::Client,
    region: String,
}

impl AwsBedrockProvider {
    /// Create a new provider, loading AWS credentials from the environment.
    pub async fn new(region: impl Into<String>) -> Self {
        let region = region.into();
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_sdk_bedrockruntime::config::Region::new(region.clone()))
            .load()
            .await;
        let client = aws_sdk_bedrockruntime::Client::new(&config);
        Self { client, region }
    }

    /// Create a provider with an existing AWS SDK client.
    pub fn with_client(client: aws_sdk_bedrockruntime::Client, region: impl Into<String>) -> Self {
        Self {
            client,
            region: region.into(),
        }
    }
}

#[async_trait]
impl LlmProvider for AwsBedrockProvider {
    fn provider_name(&self) -> &str {
        "bedrock"
    }

    fn models(&self) -> Vec<String> {
        crate::models::models_for_provider_names("bedrock")
    }

    async fn stream(
        &self,
        model: &str,
        context: LlmContext,
        options: crate::provider::StreamOptions,
        signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError> {
        let (stream, tx) = crate::streaming::AssistantMessageEventStream::new(32);
        let client = self.client.clone();
        let model = model.to_string();
        let region = self.region.clone();

        let handle = tokio::spawn(async move {
            let result =
                Self::try_stream(client, &model, context, options, &tx, signal, &region).await;
            if let Err(e) = result {
                let err_msg = e.to_string();
                let _ = tx
                    .send(crate::streaming::AssistantMessageEvent::Error {
                        error: crate::AssistantMessage {
                            content: vec![],
                            provider: "bedrock".to_string(),
                            model: model.clone(),
                            api: crate::types::Api {
                                provider: "bedrock".to_string(),
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
                            error_message: Some(format!("bedrock '{}': {}", model, err_msg,)),
                            timestamp: std::time::SystemTime::now(),
                        },
                    })
                    .await;
            }
        });

        // Detached watcher: log provider task panics
        tokio::spawn(async move {
            if let Err(e) = handle.await {
                tracing::error!(
                    provider = "bedrock",
                    error = %e,
                    "LLM provider task panicked"
                );
            }
        });

        Ok(stream)
    }
}

use crate::providers::anthropic_common as common;

impl AwsBedrockProvider {
    #[allow(clippy::too_many_arguments)]
    async fn try_stream(
        client: aws_sdk_bedrockruntime::Client,
        model: &str,
        context: LlmContext,
        options: crate::provider::StreamOptions,
        tx: &tokio::sync::mpsc::Sender<crate::streaming::AssistantMessageEvent>,
        signal: CancellationToken,
        region: &str,
    ) -> Result<(), LlmError> {
        // Build request body (Anthropic Messages API format)
        let mut messages_json = common::build_messages_json(&context.messages);
        common::apply_cache_to_last_user_message(&mut messages_json, options.cache_retention);

        let mut body = serde_json::json!({
            "anthropic_version": "bedrock-2023-05-31",
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
            let model_meta =
                crate::models::get_model("bedrock", model).unwrap_or_else(|| crate::Model {
                    id: model.to_string(),
                    name: model.to_string(),
                    api: "anthropic-messages".to_string(),
                    provider: "bedrock".to_string(),
                    base_url: format!("https://bedrock-runtime.{}.amazonaws.com", region),
                    reasoning: true,
                    input_modalities: vec![
                        crate::models::Modality::Text,
                        crate::models::Modality::Image,
                    ],
                    cost: crate::models::TokenCost {
                        input: 3.0,
                        output: 15.0,
                        cache_read: 0.3,
                        cache_write: 3.75,
                    },
                    context_window: 200_000,
                    max_tokens: 8192,
                    headers: None,
                    compat: crate::models::ModelCompat::None,
                });
            hook(&mut body, &model_meta).await;
        }

        let body_bytes =
            serde_json::to_vec(&body).map_err(|e| LlmError::Serialization(e.to_string()))?;

        // Call AWS Bedrock
        let response = client
            .invoke_model_with_response_stream()
            .model_id(model)
            .body(aws_sdk_bedrockruntime::primitives::Blob::new(body_bytes))
            .content_type("application/json")
            .send()
            .await
            .map_err(map_bedrock_sdk_error)?;

        // Invoke on_response hook (Bedrock doesn't expose HTTP headers via SDK,
        // so we pass a synthetic 200 response)
        if let Some(hook) = &options.on_response {
            let model_meta =
                crate::models::get_model("bedrock", model).unwrap_or_else(|| crate::Model {
                    id: model.to_string(),
                    name: model.to_string(),
                    api: "anthropic-messages".to_string(),
                    provider: "bedrock".to_string(),
                    base_url: format!("https://bedrock-runtime.{}.amazonaws.com", region),
                    reasoning: true,
                    input_modalities: vec![
                        crate::models::Modality::Text,
                        crate::models::Modality::Image,
                    ],
                    cost: crate::models::TokenCost {
                        input: 3.0,
                        output: 15.0,
                        cache_read: 0.3,
                        cache_write: 3.75,
                    },
                    context_window: 200_000,
                    max_tokens: 8192,
                    headers: None,
                    compat: crate::models::ModelCompat::None,
                });
            let provider_response = crate::hooks::ProviderResponse {
                status: 200,
                headers: std::collections::HashMap::new(),
            };
            hook(&provider_response, &model_meta).await;
        }

        // Process response stream
        let mut stream = response.body;
        let mut parser = common::StreamParser::new("bedrock", model);

        let _ = tx
            .send(crate::streaming::AssistantMessageEvent::Start {
                partial: parser.partial.clone(),
            })
            .await;

        loop {
            if signal.is_cancelled() {
                return Err(LlmError::Cancelled);
            }

            match stream.recv().await {
                Ok(Some(chunk)) => {
                    if let Ok(part) = chunk.as_chunk()
                        && let Some(bytes) = part.bytes()
                    {
                        let bytes = bytes.as_ref();
                        let event: serde_json::Value = serde_json::from_slice(bytes)
                            .map_err(|e| LlmError::StreamError {
                                kind: crate::StreamErrorKind::Parse,
                                message: format!("JSON parse error: {e}"),
                            })?;

                        if let Ok(Some(_)) = parser.process_event(&event, tx).await {
                            return Ok(());
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => return Err(map_bedrock_sdk_error_from_str(&e.to_string())),
            }
        }

        Ok(())
    }
}

/// Map AWS SDK Bedrock errors to LlmError.
fn map_bedrock_sdk_error<E>(err: aws_sdk_bedrockruntime::error::SdkError<E>) -> LlmError
where
    E: std::fmt::Display,
{
    map_bedrock_sdk_error_from_str(&err.to_string())
}

/// Map AWS SDK Bedrock error strings to LlmError.
fn map_bedrock_sdk_error_from_str(err_str: &str) -> LlmError {
    let err_str = err_str.to_string();

    // Check for specific error variants by string matching
    if err_str.contains("ThrottlingException") || err_str.contains("throttling") {
        return LlmError::RateLimited(err_str);
    }
    if err_str.contains("ValidationException") && err_str.contains("too long") {
        return LlmError::ContextOverflow(err_str);
    }
    if err_str.contains("ValidationException") {
        return LlmError::InvalidRequest(err_str);
    }
    if err_str.contains("AccessDeniedException") || err_str.contains("UnrecognizedClientException")
    {
        return LlmError::AuthError(err_str);
    }
    if err_str.contains("ServiceUnavailableException") || err_str.contains("ModelTimeoutException")
    {
        return LlmError::Overloaded(err_str);
    }
    if err_str.contains("ModelNotReadyException") || err_str.contains("InternalServerException") {
        return LlmError::ProviderError(err_str);
    }

    // Timeout detection
    if err_str.contains("timeout") || err_str.contains("Timed out") {
        return LlmError::Timeout(std::time::Duration::from_secs(60));
    }

    LlmError::ProviderError(err_str)
}
