use std::sync::{Arc, Mutex};

use agent_core::{AgentLoopConfig, SessionActor, SessionConfig};
use agent_core::test_utils::{AllowAllDispatcher, TestProvider};
use ai_provider::{Content, LlmContext, LlmProvider, StopReason, StreamOptions};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

fn make_compaction_actor(
    provider: Arc<dyn LlmProvider>,
) -> Arc<agent_core::harness::compaction::CompactionActor> {
    Arc::new(agent_core::harness::compaction::CompactionActor::new(
        agent_core::harness::compaction::CompactionConfig::default(),
        provider,
        "test".to_string(),
        Arc::new(agent_core::DefaultFileOperationExtractor::default()),
    ))
}

#[tokio::test]
async fn test_skill_invocation_expands_to_steer_message() {
    let _ = tracing_subscriber::fmt().try_init();

    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tmp.path().join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: A test skill\n---\n\n# Test Skill\n\nThis is the skill body.",
    )
    .unwrap();

    let skill = agent_core::Skill {
        name: "test-skill".to_string(),
        description: "A test skill".to_string(),
        file_path: skill_dir.join("SKILL.md").display().to_string(),
        base_dir: skill_dir.display().to_string(),
        source: agent_core::SkillSource::Project,
        disable_model_invocation: false,
    };

    let provider = TestProvider::text("acknowledged");
    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "test".to_string(),
        provider: provider.clone(),
        hook_dispatcher: dispatcher,
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![skill],
    });

    let result = session.prompt("/skill:test-skill".to_string()).await.unwrap();
    // result contains steer message + assistant message
    assert_eq!(result.len(), 2);

    // The steer message should have been injected and consumed.
    // Verify the session history contains the skill content.
    let msgs = session.messages();
    assert_eq!(msgs.len(), 2, "expected steer + assistant messages");

    // Find the steer message (it should be a User message before the assistant)
    let steer_msg = msgs.iter().find(|m| {
        if let ai_provider::Message::User(u) = m {
            u.content.iter().any(|c| matches!(c, Content::Text { text, .. } if text.contains("[Skill: test-skill]")))
        } else {
            false
        }
    });
    assert!(steer_msg.is_some(), "steer message with skill content should be in history");
}

#[tokio::test]
async fn test_skill_invocation_not_found_returns_error() {
    let _ = tracing_subscriber::fmt().try_init();

    let provider = TestProvider::text("acknowledged");
    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "test".to_string(),
        provider: provider.clone(),
        hook_dispatcher: dispatcher,
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    let result = session.prompt("/skill:nonexistent".to_string()).await;
    assert!(result.is_err());
    match result {
        Err(agent_core::AgentError::SkillNotFound(name)) => {
            assert_eq!(name, "nonexistent");
        }
        other => panic!("expected SkillNotFound, got {:?}", other),
    }
}

#[tokio::test]
async fn test_skills_xml_injected_into_system_prompt() {
    let _ = tracing_subscriber::fmt().try_init();

    let skill = agent_core::Skill {
        name: "rust-debug".to_string(),
        description: "Debug Rust async issues.".to_string(),
        file_path: "/skills/rust-debug/SKILL.md".to_string(),
        base_dir: "/skills/rust-debug".to_string(),
        source: agent_core::SkillSource::Project,
        disable_model_invocation: false,
    };

    struct VerifyingProvider;

    fn test_provider_config() -> &'static ai_provider::providers::shared::ProviderConfig {
        use std::sync::OnceLock;
        static CONFIG: OnceLock<ai_provider::providers::shared::ProviderConfig> = OnceLock::new();
        CONFIG.get_or_init(|| {
            ai_provider::providers::shared::ProviderConfig::new(
                None, "http://test", "test", "TEST_API_KEY",
            )
        })
    }

    #[async_trait]
    impl LlmProvider for VerifyingProvider {
        fn provider_name(&self) -> &str { "verify" }
        fn models(&self) -> Vec<String> { vec!["test".to_string()] }
        fn config(&self) -> &ai_provider::providers::shared::ProviderConfig {
            test_provider_config()
        }
        async fn stream(
            &self,
            _model: &str,
            context: LlmContext,
            _options: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<ai_provider::AssistantMessageEventStream, ai_provider::LlmError> {
            let sp = context.system_prompt.expect("system prompt should be present");
            assert!(
                sp.contains("<available_skills>"),
                "system prompt should contain <available_skills>: {}",
                sp
            );
            assert!(
                sp.contains("<name>rust-debug</name>"),
                "system prompt should contain rust-debug skill: {}",
                sp
            );
            assert!(
                sp.contains("<description>Debug Rust async issues.</description>"),
                "system prompt should contain skill description: {}",
                sp
            );

            let (stream, tx) = ai_provider::AssistantMessageEventStream::new(4);
            let partial = ai_provider::AssistantMessage {
                content: vec![Content::Text { text: "ok".to_string(), text_signature: None }],
                provider: "verify".to_string(),
                model: "test".to_string(),
                api: ai_provider::Api { provider: "verify".to_string(), model: "test".to_string() },
                usage: ai_provider::Usage {
                    input_tokens: 0, output_tokens: 0,
                    cache_creation_input_tokens: None, cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: StopReason::Stop,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            };
            tokio::spawn(async move {
                let _ = tx.send(ai_provider::AssistantMessageEvent::Start { partial: partial.clone() }).await;
                let _ = tx.send(ai_provider::AssistantMessageEvent::Done { reason: StopReason::Stop, message: partial }).await;
            });
            Ok(stream)
        }
    }

    let provider = Arc::new(VerifyingProvider);
    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "test".to_string(),
        provider: provider.clone(),
        hook_dispatcher: dispatcher,
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![skill],
    });

    let result = session.prompt("hello".to_string()).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_disabled_skill_not_in_system_prompt() {
    let _ = tracing_subscriber::fmt().try_init();

    let visible = agent_core::Skill {
        name: "visible-skill".to_string(),
        description: "I am visible.".to_string(),
        file_path: "/skills/visible/SKILL.md".to_string(),
        base_dir: "/skills/visible".to_string(),
        source: agent_core::SkillSource::Project,
        disable_model_invocation: false,
    };
    let hidden = agent_core::Skill {
        name: "hidden-skill".to_string(),
        description: "I am hidden.".to_string(),
        file_path: "/skills/hidden/SKILL.md".to_string(),
        base_dir: "/skills/hidden".to_string(),
        source: agent_core::SkillSource::Project,
        disable_model_invocation: true,
    };

    struct VerifyingProvider {
        hidden_name: String,
    }

    fn test_provider_config() -> &'static ai_provider::providers::shared::ProviderConfig {
        use std::sync::OnceLock;
        static CONFIG: OnceLock<ai_provider::providers::shared::ProviderConfig> = OnceLock::new();
        CONFIG.get_or_init(|| {
            ai_provider::providers::shared::ProviderConfig::new(
                None, "http://test", "test", "TEST_API_KEY",
            )
        })
    }

    #[async_trait]
    impl LlmProvider for VerifyingProvider {
        fn provider_name(&self) -> &str { "verify" }
        fn models(&self) -> Vec<String> { vec!["test".to_string()] }
        fn config(&self) -> &ai_provider::providers::shared::ProviderConfig {
            test_provider_config()
        }
        async fn stream(
            &self,
            _model: &str,
            context: LlmContext,
            _options: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<ai_provider::AssistantMessageEventStream, ai_provider::LlmError> {
            let sp = context.system_prompt.expect("system prompt should be present");
            assert!(
                sp.contains("visible-skill"),
                "system prompt should contain visible skill"
            );
            assert!(
                !sp.contains(&self.hidden_name),
                "system prompt should NOT contain hidden skill"
            );

            let (stream, tx) = ai_provider::AssistantMessageEventStream::new(4);
            let partial = ai_provider::AssistantMessage {
                content: vec![Content::Text { text: "ok".to_string(), text_signature: None }],
                provider: "verify".to_string(),
                model: "test".to_string(),
                api: ai_provider::Api { provider: "verify".to_string(), model: "test".to_string() },
                usage: ai_provider::Usage {
                    input_tokens: 0, output_tokens: 0,
                    cache_creation_input_tokens: None, cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: StopReason::Stop,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            };
            tokio::spawn(async move {
                let _ = tx.send(ai_provider::AssistantMessageEvent::Start { partial: partial.clone() }).await;
                let _ = tx.send(ai_provider::AssistantMessageEvent::Done { reason: StopReason::Stop, message: partial }).await;
            });
            Ok(stream)
        }
    }

    let provider = Arc::new(VerifyingProvider { hidden_name: "hidden-skill".to_string() });
    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "test".to_string(),
        provider: provider.clone(),
        hook_dispatcher: dispatcher,
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![visible, hidden],
    });

    let result = session.prompt("hello".to_string()).await;
    assert!(result.is_ok());
}


// ============================================================================
// Phase 2: Hook PromptMutation + skills preservation
// ============================================================================

#[tokio::test]
async fn test_legacy_system_prompt_replacement_preserves_skills() {
    let _ = tracing_subscriber::fmt().try_init();

    let skill = agent_core::Skill {
        name: "rust-debug".to_string(),
        description: "Debug Rust async issues.".to_string(),
        file_path: "/skills/rust-debug/SKILL.md".to_string(),
        base_dir: "/skills/rust-debug".to_string(),
        source: agent_core::SkillSource::Project,
        disable_model_invocation: false,
    };

    struct ReplacePromptDispatcher;

    #[async_trait]
    impl agent_core::HookDispatcher for ReplacePromptDispatcher {
        async fn on_before_agent_start(
            &self,
            _ctx: &agent_core::context::BeforeAgentStartCtx,
        ) -> agent_core::mutations::BeforeAgentStartMutation {
            agent_core::mutations::BeforeAgentStartMutation {
                system_prompt: Some(agent_core::prompt::PromptBuilder::from("new persona")),
                ..Default::default()
            }
        }
    }

    struct VerifyingProvider;

    fn test_provider_config() -> &'static ai_provider::providers::shared::ProviderConfig {
        use std::sync::OnceLock;
        static CONFIG: OnceLock<ai_provider::providers::shared::ProviderConfig> = OnceLock::new();
        CONFIG.get_or_init(|| {
            ai_provider::providers::shared::ProviderConfig::new(
                None, "http://test", "test", "TEST_API_KEY",
            )
        })
    }

    #[async_trait]
    impl LlmProvider for VerifyingProvider {
        fn provider_name(&self) -> &str { "verify" }
        fn models(&self) -> Vec<String> { vec!["test".to_string()] }
        fn config(&self) -> &ai_provider::providers::shared::ProviderConfig {
            test_provider_config()
        }
        async fn stream(
            &self,
            _model: &str,
            context: LlmContext,
            _options: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<ai_provider::AssistantMessageEventStream, ai_provider::LlmError> {
            let sp = context.system_prompt.expect("system prompt should be present");
            assert!(
                sp.starts_with("new persona"),
                "system prompt should start with new persona, got: {}",
                sp
            );
            assert!(
                sp.contains("<available_skills>"),
                "skills should be preserved after legacy system_prompt replacement, got: {}",
                sp
            );
            assert!(
                sp.contains("<name>rust-debug</name>"),
                "rust-debug skill should be present, got: {}",
                sp
            );

            let (stream, tx) = ai_provider::AssistantMessageEventStream::new(4);
            let partial = ai_provider::AssistantMessage {
                content: vec![Content::Text { text: "ok".to_string(), text_signature: None }],
                provider: "verify".to_string(),
                model: "test".to_string(),
                api: ai_provider::Api { provider: "verify".to_string(), model: "test".to_string() },
                usage: ai_provider::Usage {
                    input_tokens: 0, output_tokens: 0,
                    cache_creation_input_tokens: None, cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: StopReason::Stop,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            };
            tokio::spawn(async move {
                let _ = tx.send(ai_provider::AssistantMessageEvent::Start { partial: partial.clone() }).await;
                let _ = tx.send(ai_provider::AssistantMessageEvent::Done { reason: StopReason::Stop, message: partial }).await;
            });
            Ok(stream)
        }
    }

    let provider = Arc::new(VerifyingProvider);
    let dispatcher = Arc::new(ReplacePromptDispatcher);
    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "test".to_string(),
        provider: provider.clone(),
        hook_dispatcher: dispatcher,
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![skill],
    });

    let result = session.prompt("hello".to_string()).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_prompt_mutation_upsert_preserves_skills() {
    let _ = tracing_subscriber::fmt().try_init();

    let skill = agent_core::Skill {
        name: "rust-debug".to_string(),
        description: "Debug Rust async issues.".to_string(),
        file_path: "/skills/rust-debug/SKILL.md".to_string(),
        base_dir: "/skills/rust-debug".to_string(),
        source: agent_core::SkillSource::Project,
        disable_model_invocation: false,
    };

    struct UpsertFragmentDispatcher;

    #[async_trait]
    impl agent_core::HookDispatcher for UpsertFragmentDispatcher {
        async fn on_before_provider_request(
            &self,
            _ctx: &agent_core::context::ProviderRequestCtx,
        ) -> agent_core::mutations::ProviderRequestMutation {
            agent_core::mutations::ProviderRequestMutation {
                prompt_mutation: Some(agent_core::prompt::PromptMutation {
                    upsert_fragments: vec![agent_core::prompt::PromptFragment {
                        id: "custom-fragment".to_string(),
                        kind: agent_core::prompt::FragmentKind::Extension,
                        source: agent_core::prompt::FragmentSource::Extension { name: "test".to_string() },
                        content: "Custom extension text.".to_string(),
                        priority: 10,
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            }
        }
    }

    struct VerifyingProvider;

    fn test_provider_config() -> &'static ai_provider::providers::shared::ProviderConfig {
        use std::sync::OnceLock;
        static CONFIG: OnceLock<ai_provider::providers::shared::ProviderConfig> = OnceLock::new();
        CONFIG.get_or_init(|| {
            ai_provider::providers::shared::ProviderConfig::new(
                None, "http://test", "test", "TEST_API_KEY",
            )
        })
    }

    #[async_trait]
    impl LlmProvider for VerifyingProvider {
        fn provider_name(&self) -> &str { "verify" }
        fn models(&self) -> Vec<String> { vec!["test".to_string()] }
        fn config(&self) -> &ai_provider::providers::shared::ProviderConfig {
            test_provider_config()
        }
        async fn stream(
            &self,
            _model: &str,
            context: LlmContext,
            _options: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<ai_provider::AssistantMessageEventStream, ai_provider::LlmError> {
            let sp = context.system_prompt.expect("system prompt should be present");
            assert!(
                sp.contains("Custom extension text."),
                "custom fragment should be present, got: {}",
                sp
            );
            assert!(
                sp.contains("<available_skills>"),
                "skills should be preserved after prompt_mutation upsert, got: {}",
                sp
            );
            assert!(
                sp.contains("<name>rust-debug</name>"),
                "rust-debug skill should be present, got: {}",
                sp
            );

            let (stream, tx) = ai_provider::AssistantMessageEventStream::new(4);
            let partial = ai_provider::AssistantMessage {
                content: vec![Content::Text { text: "ok".to_string(), text_signature: None }],
                provider: "verify".to_string(),
                model: "test".to_string(),
                api: ai_provider::Api { provider: "verify".to_string(), model: "test".to_string() },
                usage: ai_provider::Usage {
                    input_tokens: 0, output_tokens: 0,
                    cache_creation_input_tokens: None, cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: StopReason::Stop,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            };
            tokio::spawn(async move {
                let _ = tx.send(ai_provider::AssistantMessageEvent::Start { partial: partial.clone() }).await;
                let _ = tx.send(ai_provider::AssistantMessageEvent::Done { reason: StopReason::Stop, message: partial }).await;
            });
            Ok(stream)
        }
    }

    let provider = Arc::new(VerifyingProvider);
    let dispatcher = Arc::new(UpsertFragmentDispatcher);
    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "test".to_string(),
        provider: provider.clone(),
        hook_dispatcher: dispatcher,
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![skill],
    });

    let result = session.prompt("hello".to_string()).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_prompt_mutation_can_remove_skills() {
    let _ = tracing_subscriber::fmt().try_init();

    let skill = agent_core::Skill {
        name: "rust-debug".to_string(),
        description: "Debug Rust async issues.".to_string(),
        file_path: "/skills/rust-debug/SKILL.md".to_string(),
        base_dir: "/skills/rust-debug".to_string(),
        source: agent_core::SkillSource::Project,
        disable_model_invocation: false,
    };

    struct RemoveSkillsDispatcher;

    #[async_trait]
    impl agent_core::HookDispatcher for RemoveSkillsDispatcher {
        async fn on_before_provider_request(
            &self,
            _ctx: &agent_core::context::ProviderRequestCtx,
        ) -> agent_core::mutations::ProviderRequestMutation {
            agent_core::mutations::ProviderRequestMutation {
                prompt_mutation: Some(agent_core::prompt::PromptMutation {
                    remove_kinds: vec![agent_core::prompt::FragmentKind::SkillsDirectory],
                    ..Default::default()
                }),
                ..Default::default()
            }
        }
    }

    struct VerifyingProvider;

    fn test_provider_config() -> &'static ai_provider::providers::shared::ProviderConfig {
        use std::sync::OnceLock;
        static CONFIG: OnceLock<ai_provider::providers::shared::ProviderConfig> = OnceLock::new();
        CONFIG.get_or_init(|| {
            ai_provider::providers::shared::ProviderConfig::new(
                None, "http://test", "test", "TEST_API_KEY",
            )
        })
    }

    #[async_trait]
    impl LlmProvider for VerifyingProvider {
        fn provider_name(&self) -> &str { "verify" }
        fn models(&self) -> Vec<String> { vec!["test".to_string()] }
        fn config(&self) -> &ai_provider::providers::shared::ProviderConfig {
            test_provider_config()
        }
        async fn stream(
            &self,
            _model: &str,
            context: LlmContext,
            _options: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<ai_provider::AssistantMessageEventStream, ai_provider::LlmError> {
            let sp = context.system_prompt.expect("system prompt should be present");
            assert!(
                !sp.contains("<available_skills>"),
                "skills should be explicitly removed by prompt_mutation, got: {}",
                sp
            );
            assert!(
                !sp.contains("rust-debug"),
                "rust-debug skill should not be present, got: {}",
                sp
            );

            let (stream, tx) = ai_provider::AssistantMessageEventStream::new(4);
            let partial = ai_provider::AssistantMessage {
                content: vec![Content::Text { text: "ok".to_string(), text_signature: None }],
                provider: "verify".to_string(),
                model: "test".to_string(),
                api: ai_provider::Api { provider: "verify".to_string(), model: "test".to_string() },
                usage: ai_provider::Usage {
                    input_tokens: 0, output_tokens: 0,
                    cache_creation_input_tokens: None, cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: StopReason::Stop,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            };
            tokio::spawn(async move {
                let _ = tx.send(ai_provider::AssistantMessageEvent::Start { partial: partial.clone() }).await;
                let _ = tx.send(ai_provider::AssistantMessageEvent::Done { reason: StopReason::Stop, message: partial }).await;
            });
            Ok(stream)
        }
    }

    let provider = Arc::new(VerifyingProvider);
    let dispatcher = Arc::new(RemoveSkillsDispatcher);
    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "test".to_string(),
        provider: provider.clone(),
        hook_dispatcher: dispatcher,
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: None,
        skills: vec![skill],
    });

    let result = session.prompt("hello".to_string()).await;
    assert!(result.is_ok());
}
