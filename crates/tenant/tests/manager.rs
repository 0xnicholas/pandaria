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
use agent_core::{RuntimeConfig, CompactionConfig, AgentSpace, DefaultHookConfig};

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
            // Respect cancellation token
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
async fn test_manager_create_session() {
    let _ = tracing_subscriber::fmt().try_init();

    let registry = Arc::new(TenantRegistry::new());
    let tenant = Tenant::new("t1", TenantQuota::default());
    registry.register(tenant).unwrap();

    let provider: Arc<dyn LlmProvider> = Arc::new(EchoProvider::new());
    let manager = {
        let runtime_config = Arc::new(RuntimeConfig {
            provider: provider.clone(),
            default_model: "echo".to_string(),
            default_system_prompt: "You are helpful.".to_string(),
            default_context_window: 128_000,
            store: None,
            media_provider: None,
            media_registry: None,
            http_client: reqwest::Client::new(),
            compaction_config: CompactionConfig::default(),
            agent_space: AgentSpace::default(),
            hook_config: DefaultHookConfig::default(),
            memory_store: None,
        });
        TenantManagerImpl::new(registry, runtime_config)
    };

    let info = manager
        .create_session("t1", CreateSessionParams::default())
        .await
        .unwrap();

    assert_eq!(info.tenant_id, "t1");
    assert!(!info.id.to_string().is_empty());

    // Clean up
    manager.delete_session("t1", &info.id).await.unwrap();
}

#[tokio::test]
async fn test_manager_create_session_unknown_tenant() {
    let registry = Arc::new(TenantRegistry::new());
    let provider: Arc<dyn LlmProvider> = Arc::new(EchoProvider::new());
    let manager = {
        let runtime_config = Arc::new(RuntimeConfig {
            provider: provider.clone(),
            default_model: "echo".to_string(),
            default_system_prompt: "You are helpful.".to_string(),
            default_context_window: 128_000,
            store: None,
            media_provider: None,
            media_registry: None,
            http_client: reqwest::Client::new(),
            compaction_config: CompactionConfig::default(),
            agent_space: AgentSpace::default(),
            hook_config: DefaultHookConfig::default(),
            memory_store: None,
        });
        TenantManagerImpl::new(registry, runtime_config)
    };

    let err = manager
        .create_session("unknown", CreateSessionParams::default())
        .await
        .unwrap_err();

    assert!(matches!(err, tenant::TenantError::TenantNotFound(_)));
}

#[tokio::test]
async fn test_manager_list_and_get_session() {
    let _ = tracing_subscriber::fmt().try_init();

    let registry = Arc::new(TenantRegistry::new());
    let tenant = Tenant::new("t1", TenantQuota::default());
    registry.register(tenant).unwrap();

    let provider: Arc<dyn LlmProvider> = Arc::new(EchoProvider::new());
    let manager = {
        let runtime_config = Arc::new(RuntimeConfig {
            provider: provider.clone(),
            default_model: "echo".to_string(),
            default_system_prompt: "You are helpful.".to_string(),
            default_context_window: 128_000,
            store: None,
            media_provider: None,
            media_registry: None,
            http_client: reqwest::Client::new(),
            compaction_config: CompactionConfig::default(),
            agent_space: AgentSpace::default(),
            hook_config: DefaultHookConfig::default(),
            memory_store: None,
        });
        TenantManagerImpl::new(registry, runtime_config)
    };

    let info = manager
        .create_session("t1", CreateSessionParams::default())
        .await
        .unwrap();

    let sessions = manager.list_sessions("t1").await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, info.id);
    assert!(!sessions[0].created_at.is_empty());

    let got = manager.get_session("t1", &info.id).await.unwrap();
    assert_eq!(got.id, info.id);
    assert_eq!(got.created_at, info.created_at);

    manager.delete_session("t1", &info.id).await.unwrap();
}

#[tokio::test]
async fn test_manager_send_message() {
    let _ = tracing_subscriber::fmt().try_init();

    let registry = Arc::new(TenantRegistry::new());
    let tenant = Tenant::new("t1", TenantQuota::default());
    registry.register(tenant).unwrap();

    let provider: Arc<dyn LlmProvider> = Arc::new(EchoProvider::new());
    let manager = {
        let runtime_config = Arc::new(RuntimeConfig {
            provider: provider.clone(),
            default_model: "echo".to_string(),
            default_system_prompt: "You are helpful.".to_string(),
            default_context_window: 128_000,
            store: None,
            media_provider: None,
            media_registry: None,
            http_client: reqwest::Client::new(),
            compaction_config: CompactionConfig::default(),
            agent_space: AgentSpace::default(),
            hook_config: DefaultHookConfig::default(),
            memory_store: None,
        });
        TenantManagerImpl::new(registry, runtime_config)
    };

    let info = manager
        .create_session("t1", CreateSessionParams::default())
        .await
        .unwrap();

    let turn_index = manager
        .send_message("t1", &info.id, vec![ai_provider::Content::Text { text: "hello".to_string(), text_signature: None }])
        .await
        .unwrap();

    assert_eq!(turn_index, 0);

    manager.delete_session("t1", &info.id).await.unwrap();
}

#[tokio::test]
async fn test_manager_subscribe_events() {
    let _ = tracing_subscriber::fmt().try_init();

    let registry = Arc::new(TenantRegistry::new());
    let tenant = Tenant::new("t1", TenantQuota::default());
    registry.register(tenant).unwrap();

    let provider: Arc<dyn LlmProvider> = Arc::new(EchoProvider::new());
    let manager = {
        let runtime_config = Arc::new(RuntimeConfig {
            provider: provider.clone(),
            default_model: "echo".to_string(),
            default_system_prompt: "You are helpful.".to_string(),
            default_context_window: 128_000,
            store: None,
            media_provider: None,
            media_registry: None,
            http_client: reqwest::Client::new(),
            compaction_config: CompactionConfig::default(),
            agent_space: AgentSpace::default(),
            hook_config: DefaultHookConfig::default(),
            memory_store: None,
        });
        TenantManagerImpl::new(registry, runtime_config)
    };

    let info = manager
        .create_session("t1", CreateSessionParams::default())
        .await
        .unwrap();

    let mut rx = manager.subscribe_events("t1", &info.id).await.unwrap();

    // Send a message to trigger events
    let _ = manager
        .send_message("t1", &info.id, vec![ai_provider::Content::Text { text: "hello".to_string(), text_signature: None }])
        .await
        .unwrap();

    // Wait for at least one event with timeout
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .unwrap();

    assert!(event.is_some());

    manager.delete_session("t1", &info.id).await.unwrap();
}

#[tokio::test]
async fn test_manager_delete_session_releases_slot() {
    let _ = tracing_subscriber::fmt().try_init();

    let registry = Arc::new(TenantRegistry::new());
    let tenant = Tenant::new("t1", TenantQuota {
        max_concurrent_sessions: 1,
        ..TenantQuota::default()
    });
    registry.register(tenant).unwrap();

    let provider: Arc<dyn LlmProvider> = Arc::new(EchoProvider::new());
    let manager = {
        let runtime_config = Arc::new(RuntimeConfig {
            provider: provider.clone(),
            default_model: "echo".to_string(),
            default_system_prompt: "You are helpful.".to_string(),
            default_context_window: 128_000,
            store: None,
            media_provider: None,
            media_registry: None,
            http_client: reqwest::Client::new(),
            compaction_config: CompactionConfig::default(),
            agent_space: AgentSpace::default(),
            hook_config: DefaultHookConfig::default(),
            memory_store: None,
        });
        TenantManagerImpl::new(registry.clone(), runtime_config)
    };

    let info = manager
        .create_session("t1", CreateSessionParams::default())
        .await
        .unwrap();

    // Delete the session
    manager.delete_session("t1", &info.id).await.unwrap();

    // Should be able to create another session (slot released)
    let info2 = manager
        .create_session("t1", CreateSessionParams::default())
        .await
        .unwrap();

    assert_ne!(info.id, info2.id);

    manager.delete_session("t1", &info2.id).await.unwrap();
}

#[tokio::test]
async fn test_manager_interrupt_does_not_deadlock() {
    let _ = tracing_subscriber::fmt().try_init();

    let registry = Arc::new(TenantRegistry::new());
    let tenant = Tenant::new("t1", TenantQuota::default());
    registry.register(tenant).unwrap();

    let provider: Arc<dyn LlmProvider> = Arc::new(EchoProvider::new());
    let manager = {
        let runtime_config = Arc::new(RuntimeConfig {
            provider: provider.clone(),
            default_model: "echo".to_string(),
            default_system_prompt: "You are helpful.".to_string(),
            default_context_window: 128_000,
            store: None,
            media_provider: None,
            media_registry: None,
            http_client: reqwest::Client::new(),
            compaction_config: CompactionConfig::default(),
            agent_space: AgentSpace::default(),
            hook_config: DefaultHookConfig::default(),
            memory_store: None,
        });
        TenantManagerImpl::new(registry, runtime_config)
    };

    let info = manager
        .create_session("t1", CreateSessionParams::default())
        .await
        .unwrap();

    // Interrupt should complete immediately without needing the actor lock
    manager.interrupt("t1", &info.id).await.unwrap();

    manager.delete_session("t1", &info.id).await.unwrap();
}
