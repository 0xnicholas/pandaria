use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::provider::LlmProvider;
use crate::streaming::AssistantMessageEventStream;
use crate::types::LlmContext;

#[allow(dead_code)]
pub struct AwsBedrockProvider {
    client: aws_sdk_bedrockruntime::Client,
    region: String,
}

impl AwsBedrockProvider {
    pub async fn new(region: impl Into<String>) -> Self {
        let region = region.into();
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_sdk_bedrockruntime::config::Region::new(region.clone()))
            .load()
            .await;
        let client = aws_sdk_bedrockruntime::Client::new(&config);
        Self { client, region }
    }

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
        vec![
            "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
            "anthropic.claude-3-opus-20240229-v1:0".to_string(),
            "anthropic.claude-3-haiku-20240307-v1:0".to_string(),
        ]
    }

    async fn stream(
        &self,
        _model: &str,
        _context: LlmContext,
        _options: crate::provider::StreamOptions,
        _signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError> {
        let (_stream, _tx) = AssistantMessageEventStream::new(32);
        // TODO: Implement Bedrock ConverseStream integration
        // This is a placeholder - full implementation will handle:
        // - Message conversion (system, user, assistant, tool_result)
        // - Tool configuration
        // - Cache control support
        // - Reasoning configuration
        // - ConverseStream event mapping to AssistantMessageEvent
        Err(LlmError::ProviderError(
            "Bedrock provider not yet fully implemented".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_name_static() {
        // Verify the provider name without constructing the provider
        assert_eq!("bedrock", "bedrock");
    }
}
