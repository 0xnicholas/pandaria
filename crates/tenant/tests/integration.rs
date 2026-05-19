use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use ai_provider::{
    Api, AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream,
    Content, LlmContext, LlmError, LlmProvider, StopReason, StreamOptions, Usage,
    providers::shared::ProviderConfig,
};
use tokio_util::sync::CancellationToken;

use tenant::manager::{CreateSessionParams, TenantManager, TenantManagerImpl};
use tenant::{Tenant, TenantQuota, TenantRegistry};

struct EchoProvider {
    config: ProviderConfig,
}

impl EchoProvider {
    fn new() -> Self {
        Self {
            config: ProviderConfig::new(
                None,
                "http://localhost:9999",
                "echo",
                "ECHO_API_KEY",
            ),
        }
    }
}

#[async_trait]
impl LlmProvider for EchoProvider {
    fn provider_name(&self) -> &str {
        "echo"
    }

    fn models(&self) -> Vec<String> {
        vec!["echo".to_string()]
    }

    fn config(&self) -> &ProviderConfig {
        &self.config
    }

    async fn stream(
        &self,
        _model: &str,
        _context: LlmContext,
        _options: StreamOptions,
        signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError> {
        let (stream, tx) = AssistantMessageEventStream::new(4);
        let msg = AssistantMessage {
            content: vec![Content::Text {
                text: "hi".to_string(),
                text_signature: None,
            }],
            provider: "echo".to_string(),
            model: "echo".to_string(),
            api: Api {
                provider: "echo".to_string(),
                model: "echo".to_string(),
            },
            usage: Usage {
                input_tokens: 5,
                output_tokens: 5,
                total_tokens: 10,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
            stop_reason: StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: SystemTime::now(),
        };
        tokio::spawn(async move {
            if signal.is_cancelled() {
                return;
            }
            let _ = tx
                .send(AssistantMessageEvent::Start {
                    partial: msg.clone(),
                })
                .await;
            let _ = tx
                .send(AssistantMessageEvent::Done {
                    reason: StopReason::Stop,
                    message: msg,
                })
                .await;
        });
        Ok(stream)
    }
}

#[tokio::test]
async fn test_end_to_end_tenant_isolation() {
    let _ = tracing_subscriber::fmt().try_init();

    let registry = Arc::new(TenantRegistry::new());

    let t1 = Tenant::new("t1", TenantQuota {
        max_concurrent_sessions: 2,
        max_tokens_per_day: 100,
        max_tool_calls_per_minute: 10,
        ..TenantQuota::default()
    });
    let t2 = Tenant::new("t2", TenantQuota {
        max_concurrent_sessions: 5,
        max_tokens_per_day: 200,
        max_tool_calls_per_minute: 20,
        ..TenantQuota::default()
    });

    registry.register(t1).unwrap();
    registry.register(t2).unwrap();

    let provider: Arc<dyn LlmProvider> = Arc::new(EchoProvider::new());

    let manager = TenantManagerImpl::new(
        registry.clone(),
        provider,
        None,
        "echo",
        "You are helpful.",
        128_000,
    );

    // Tenant 1
    let info1 = manager
        .create_session("t1", CreateSessionParams { title: None, system_prompt: None })
        .await
        .unwrap();

    manager
        .send_message("t1", &info1.id, "hello".to_string())
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Tenant 2
    let info2 = manager
        .create_session("t2", CreateSessionParams { title: None, system_prompt: None })
        .await
        .unwrap();

    manager
        .send_message("t2", &info2.id, "world".to_string())
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Clean up
    manager.delete_session("t1", &info1.id).await.unwrap();
    manager.delete_session("t2", &info2.id).await.unwrap();
}

#[tokio::test]
async fn test_tenant_session_limit_enforced() {
    let _ = tracing_subscriber::fmt().try_init();

    let registry = Arc::new(TenantRegistry::new());
    let t1 = Tenant::new("t1", TenantQuota {
        max_concurrent_sessions: 1,
        ..TenantQuota::default()
    });
    registry.register(t1).unwrap();

    let provider: Arc<dyn LlmProvider> = Arc::new(EchoProvider::new());
    let manager = TenantManagerImpl::new(
        registry.clone(),
        provider,
        None,
        "echo",
        "You are helpful.",
        128_000,
    );

    let info1 = manager
        .create_session("t1", CreateSessionParams { title: None, system_prompt: None })
        .await
        .unwrap();

    let err = manager
        .create_session("t1", CreateSessionParams { title: None, system_prompt: None })
        .await
        .unwrap_err();

    assert!(matches!(err, tenant::TenantError::SessionLimitExceeded { .. }));

    manager.delete_session("t1", &info1.id).await.unwrap();
}

#[tokio::test]
async fn test_delete_session_not_found() {
    let registry = Arc::new(TenantRegistry::new());
    let provider: Arc<dyn LlmProvider> = Arc::new(EchoProvider::new());
    let manager = TenantManagerImpl::new(
        registry,
        provider,
        None,
        "echo",
        "You are helpful.",
        128_000,
    );

    let fake_id = uuid::Uuid::new_v4();
    let err = manager.delete_session("t1", &fake_id).await.unwrap_err();
    assert!(matches!(err, tenant::TenantError::SessionNotFound(_)));
}

#[tokio::test]
async fn test_shutdown_cleans_all_sessions() {
    let _ = tracing_subscriber::fmt().try_init();

    let registry = Arc::new(TenantRegistry::new());
    let t1 = Tenant::new("t1", TenantQuota::default());
    registry.register(t1).unwrap();

    let provider: Arc<dyn LlmProvider> = Arc::new(EchoProvider::new());
    let manager = TenantManagerImpl::new(
        registry.clone(),
        provider,
        None,
        "echo",
        "You are helpful.",
        128_000,
    );

    let info1 = manager
        .create_session("t1", CreateSessionParams { title: None, system_prompt: None })
        .await
        .unwrap();
    let info2 = manager
        .create_session("t1", CreateSessionParams { title: None, system_prompt: None })
        .await
        .unwrap();

    assert_eq!(manager.list_sessions("t1").await.unwrap().len(), 2);

    manager.shutdown().await;

    // After shutdown, sessions should be removed
    assert!(manager.list_sessions("t1").await.unwrap().is_empty());

    // And slots should be released
    let t1_sv = registry.get("t1").unwrap();
    assert_eq!(t1_sv.quota_status().active_sessions, 0);
}
