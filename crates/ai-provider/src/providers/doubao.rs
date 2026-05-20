use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::streaming::AssistantMessageEvent;
use crate::types::LlmContext;

crate::providers::shared::define_provider!(
    DoubaoProvider,
    "doubao",
    "DOUBAO_API_KEY",
    "https://ark.cn-beijing.volces.com/api/v3/chat/completions"
);

impl DoubaoProvider {
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
            client, base_url, model, context, options, tx, api_key, signal, "doubao",
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
        let p = DoubaoProvider::new(None);
        assert_eq!(p.provider_name(), "doubao");
    }

    #[test]
    fn test_models() {
        let p = DoubaoProvider::new(None);
        let m = p.models();
        assert!(m.contains(&"doubao-seed-2.0-pro".to_string()));
        assert!(m.contains(&"doubao-seed-2.0-lite".to_string()));
        assert!(m.contains(&"doubao-seed-1.6".to_string()));
    }
}
