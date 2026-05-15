use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::streaming::AssistantMessageEvent;
use crate::types::LlmContext;

crate::providers::shared::define_provider!(
    DeepSeekProvider,
    "deepseek",
    "DEEPSEEK_API_KEY",
    "https://api.deepseek.com/chat/completions"
);

impl DeepSeekProvider {
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
        crate::providers::openai::openai_compatible_stream(
            client, base_url, model, context, options, tx, api_key, signal, "deepseek",
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::LlmProvider;

    #[test]
    fn test_provider_name() {
        let p = DeepSeekProvider::new(None);
        assert_eq!(p.provider_name(), "deepseek");
    }

    #[test]
    fn test_models() {
        let p = DeepSeekProvider::new(None);
        let m = p.models();
        assert!(m.contains(&"deepseek-chat".to_string()));
        assert!(m.contains(&"deepseek-reasoner".to_string()));
    }
}
