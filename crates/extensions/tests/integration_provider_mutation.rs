use std::sync::Arc;

use agent_core::compaction::{CompactionActor, CompactionConfig};
use agent_core::context::{ProviderRequestCtx, ProviderResponseCtx};
use agent_core::file_ops::DefaultFileOperationExtractor;
use agent_core::mutations::{ProviderRequestMutation, ProviderResponseMutation};
use agent_core::SessionActor;
use agent_core::test_utils::TestProvider;
use agent_core::types::AgentMessage;
use async_trait::async_trait;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;
use ai_provider::{
    Api, AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, Content, LlmContext,
    LlmProvider, StopReason, StreamOptions, Usage,
};
use tokio_util::sync::CancellationToken;

fn make_compaction_actor(provider: Arc<dyn ai_provider::LlmProvider>) -> Arc<CompactionActor> {
    Arc::new(CompactionActor::new(
        CompactionConfig::default(),
        provider,
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    ))
}

// ============================================================================
// MutateRequestExt — mutates the provider request system_prompt
// ============================================================================

struct MutateRequestExt;

#[async_trait]
impl Extension for MutateRequestExt {
    fn name(&self) -> &str {
        "mutate_request"
    }

    async fn on_before_provider_request(
        &self,
        _ctx: &ProviderRequestCtx,
    ) -> ProviderRequestMutation {
        ProviderRequestMutation {
            system_prompt: Some(Some("mutated_prompt".to_string())),
            ..Default::default()
        }
    }
}

// ============================================================================
// VerifyProvider — asserts the system_prompt was mutated
// ============================================================================

struct VerifyProvider {
    expected_system_prompt: Option<String>,
}

#[async_trait]
impl LlmProvider for VerifyProvider {
    fn provider_name(&self) -> &str {
        "verify"
    }

    fn models(&self) -> Vec<String> {
        vec!["verify".to_string()]
    }

    fn config(&self) -> &ai_provider::providers::shared::ProviderConfig {
        use std::sync::OnceLock;
        static CONFIG: OnceLock<ai_provider::providers::shared::ProviderConfig> = OnceLock::new();
        CONFIG.get_or_init(|| {
            ai_provider::providers::shared::ProviderConfig::new(
                None, "http://mock", "verify", "VERIFY_API_KEY",
            )
        })
    }

    async fn stream(
        &self,
        _model: &str,
        context: LlmContext,
        _options: StreamOptions,
        _signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, ai_provider::LlmError> {
        if let Some(ref expected) = self.expected_system_prompt {
            assert_eq!(context.system_prompt, Some(expected.clone()));
        }

        let (stream, tx) = AssistantMessageEventStream::new(4);
        let partial = AssistantMessage {
            content: vec![Content::Text {
                text: "ok".to_string(),
                text_signature: None,
            }],
            provider: "verify".to_string(),
            model: "verify".to_string(),
            api: Api {
                provider: "verify".to_string(),
                model: "verify".to_string(),
            },
            usage: Usage {
                input_tokens: 0,
                output_tokens: 1,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 1,
            },
            stop_reason: StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        };

        tokio::spawn(async move {
            let _ = tx
                .send(AssistantMessageEvent::Start {
                    partial: partial.clone(),
                })
                .await;
            let _ = tx
                .send(AssistantMessageEvent::Done {
                    reason: StopReason::Stop,
                    message: partial,
                })
                .await;
        });

        Ok(stream)
    }
}

// ============================================================================
// MutateResponseExt — mutates the provider response content
// ============================================================================

struct MutateResponseExt;

#[async_trait]
impl Extension for MutateResponseExt {
    fn name(&self) -> &str {
        "mutate_response"
    }

    async fn on_after_provider_response(
        &self,
        _ctx: &ProviderResponseCtx,
    ) -> ProviderResponseMutation {
        ProviderResponseMutation {
            content: Some(vec![Content::Text {
                text: "mutated_response".to_string(),
                text_signature: None,
            }]),
            ..Default::default()
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
async fn test_provider_request_mutation() {
    let _ = tracing_subscriber::fmt().try_init();

    let ext = Arc::new(MutateRequestExt);
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);

    let provider = Arc::new(VerifyProvider {
        expected_system_prompt: Some("mutated_prompt".to_string()),
    });
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "original_prompt".to_string(),
        "verify".to_string(),
        provider.clone(),
        Arc::new(router),
        make_compaction_actor(provider),
        vec![],
        None,
    vec![],
    );

    let results = session.prompt("hello".to_string()).await.unwrap();
    assert!(!results.is_empty());
}

#[tokio::test]
async fn test_provider_response_mutation() {
    let _ = tracing_subscriber::fmt().try_init();

    let ext = Arc::new(MutateResponseExt);
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);

    let provider = TestProvider::text("original");
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "prompt".to_string(),
        "test".to_string(),
        provider.clone(),
        Arc::new(router),
        make_compaction_actor(provider),
        vec![],
        None,
    vec![],
    );

    let results = session.prompt("hello".to_string()).await.unwrap();
    assert!(!results.is_empty());

    match &results[0] {
        AgentMessage::Assistant(msg) => {
            let text = msg
                .content
                .iter()
                .filter_map(|c| match c {
                    Content::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            assert_eq!(text, "mutated_response");
        }
        _ => panic!("expected assistant message"),
    }
}
