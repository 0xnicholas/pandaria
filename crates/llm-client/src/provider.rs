use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::streaming::AssistantMessageEventStream;
use crate::types::LlmContext;

#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn provider_name(&self) -> &str;

    fn models(&self) -> Vec<String>;

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
    use async_trait::async_trait;
    use futures::StreamExt;
    use crate::Content;
    use crate::Api;
    use crate::Usage;
    use crate::StopReason;
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

        async fn stream(
            &self,
            _model: &str,
            _context: LlmContext,
            _options: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<AssistantMessageEventStream, LlmError> {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(AssistantMessageEvent::Start),
                Ok(AssistantMessageEvent::TextDelta {
                    text: "Hello".to_string(),
                }),
                Ok(AssistantMessageEvent::Done {
                    content: vec![Content::Text {
                        text: "Hello".to_string(),
                    }],
                    api: Api {
                        provider: "mock".to_string(),
                        model: "mock-v1".to_string(),
                    },
                    usage: Usage {
                        input_tokens: 0,
                        output_tokens: 5,
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
                    },
                    stop_reason: StopReason::Stop,
                }),
            ])))
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
            .stream("mock-v1", ctx, StreamOptions::default(), CancellationToken::new())
            .await
            .unwrap();
        let event = stream.next().await.unwrap().unwrap();
        assert!(matches!(event, AssistantMessageEvent::Start));

        let event = stream.next().await.unwrap().unwrap();
        match event {
            AssistantMessageEvent::TextDelta { text } => assert_eq!(text, "Hello"),
            _ => panic!("expected TextDelta"),
        }

        let event = stream.next().await.unwrap().unwrap();
        match event {
            AssistantMessageEvent::Done { stop_reason, .. } => {
                assert_eq!(stop_reason, StopReason::Stop);
            }
            _ => panic!("expected Done"),
        }
    }
}
