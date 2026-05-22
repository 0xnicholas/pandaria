use std::sync::{Arc, Mutex};

use agent_core::harness::agent_loop::resolve_orphan_tool_calls;
use agent_core::prompt::PromptBuilder;
use agent_core::test_utils::{AllowAllDispatcher, TestProvider};
use agent_core::{
    AgentEvent, AgentLoop, AgentLoopConfig, HookDispatcher, SessionActor, SessionConfig,
};
use ai_provider::{Content, LlmContext, LlmProvider, StopReason, StreamOptions, ToolCall};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

fn make_loop_config(
    provider: Arc<dyn LlmProvider>,
    dispatcher: Arc<dyn HookDispatcher>,
    tools: Vec<agent_core::AgentToolRef>,
) -> AgentLoopConfig {
    AgentLoopConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        model: "test".to_string(),
        provider,
        hook_dispatcher: dispatcher,
        tools,
        prompt_builder: PromptBuilder::from("You are helpful."),
        stream_options: StreamOptions::default(),
        steer_queue: Arc::new(Mutex::new(vec![])),
        follow_up_queue: Arc::new(Mutex::new(vec![])),
        event_sink: Arc::new(|event| {
            tracing::debug!("event: {:?}", event);
        }),
        circuit_breaker: None,
        skills: Vec::new(),
    }
}

#[tokio::test]
async fn test_follow_up_triggers_second_turn() {
    let _ = tracing_subscriber::fmt().try_init();

    let provider = TestProvider::counted(|n| {
        agent_core::test_utils::TestResponse::Text(format!("response{}", n))
    });
    let dispatcher = Arc::new(AllowAllDispatcher);
    let follow_up_queue = Arc::new(Mutex::new(vec![ai_provider::Message::User(
        ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "follow_up_msg".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        },
    )]));
    let config = AgentLoopConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        model: "test".to_string(),
        provider,
        hook_dispatcher: dispatcher,
        tools: vec![],
        prompt_builder: PromptBuilder::from("You are helpful."),
        stream_options: StreamOptions::default(),
        steer_queue: Arc::new(Mutex::new(vec![])),
        follow_up_queue: follow_up_queue.clone(),
        event_sink: Arc::new(|event| {
            tracing::debug!("event: {:?}", event);
        }),
        circuit_breaker: None,
        skills: Vec::new(),
    };
    let loop_ = AgentLoop::new(config);

    let user_msg = ai_provider::Message::User(ai_provider::UserMessage {
        content: vec![Content::Text {
            text: "hi".to_string(),
            text_signature: None,
        }],
        timestamp: std::time::SystemTime::now(),
    });

    let results = loop_
        .run(vec![user_msg], CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(results.len(), 3);
    assert!(matches!(&results[0], ai_provider::Message::Assistant(_)));
    assert!(matches!(&results[1], ai_provider::Message::User(_)));
    assert!(matches!(&results[2], ai_provider::Message::Assistant(_)));
    assert!(follow_up_queue.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_steer_injection() {
    let _ = tracing_subscriber::fmt().try_init();

    // VerifyingProvider checks that the steer message appears in the LLM context.
    struct VerifyingProvider {
        expected_text: String,
    }
    fn test_provider_config() -> &'static ai_provider::providers::shared::ProviderConfig {
        use std::sync::OnceLock;
        static CONFIG: OnceLock<ai_provider::providers::shared::ProviderConfig> = OnceLock::new();
        CONFIG.get_or_init(|| {
            ai_provider::providers::shared::ProviderConfig::new(
                None,
                "http://test",
                "test",
                "TEST_API_KEY",
            )
        })
    }

    #[async_trait]
    impl LlmProvider for VerifyingProvider {
        fn provider_name(&self) -> &str {
            "verify"
        }
        fn models(&self) -> Vec<String> {
            vec!["test".to_string()]
        }
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
            let has_steer = context.messages.iter().any(|m| {
                if let ai_provider::Message::User(u) = m {
                    u.content.iter().any(
                        |c| matches!(c, Content::Text { text, .. } if text == &self.expected_text),
                    )
                } else {
                    false
                }
            });
            assert!(has_steer, "steer message should appear in LLM context");

            let (stream, tx) = ai_provider::AssistantMessageEventStream::new(4);
            let partial = ai_provider::AssistantMessage {
                content: vec![Content::Text {
                    text: "ok".to_string(),
                    text_signature: None,
                }],
                provider: "verify".to_string(),
                model: "test".to_string(),
                api: ai_provider::Api {
                    provider: "verify".to_string(),
                    model: "test".to_string(),
                },
                usage: ai_provider::Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: StopReason::Stop,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            };
            tokio::spawn(async move {
                let _ = tx
                    .send(ai_provider::AssistantMessageEvent::Start {
                        partial: partial.clone(),
                    })
                    .await;
                let _ = tx
                    .send(ai_provider::AssistantMessageEvent::Done {
                        reason: StopReason::Stop,
                        message: partial,
                    })
                    .await;
            });
            Ok(stream)
        }
    }

    let provider = Arc::new(VerifyingProvider {
        expected_text: "steer_msg".to_string(),
    });
    let dispatcher = Arc::new(AllowAllDispatcher);
    let steer_queue = Arc::new(Mutex::new(vec![ai_provider::Message::User(
        ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "steer_msg".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        },
    )]));
    let config = AgentLoopConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        model: "test".to_string(),
        provider,
        hook_dispatcher: dispatcher,
        tools: vec![],
        prompt_builder: PromptBuilder::from("You are helpful."),
        stream_options: StreamOptions::default(),
        steer_queue: steer_queue.clone(),
        follow_up_queue: Arc::new(Mutex::new(vec![])),
        event_sink: Arc::new(|event| {
            tracing::debug!("event: {:?}", event);
        }),
        circuit_breaker: None,
        skills: Vec::new(),
    };
    let loop_ = AgentLoop::new(config);

    let user_msg = ai_provider::Message::User(ai_provider::UserMessage {
        content: vec![Content::Text {
            text: "hi".to_string(),
            text_signature: None,
        }],
        timestamp: std::time::SystemTime::now(),
    });

    let results = loop_
        .run(vec![user_msg], CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert!(matches!(&results[0], ai_provider::Message::User(_)));
    assert!(matches!(&results[1], ai_provider::Message::Assistant(_)));
    assert!(steer_queue.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_event_sequence() {
    let _ = tracing_subscriber::fmt().try_init();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();

    let provider = TestProvider::text("Hello!");
    let dispatcher = Arc::new(AllowAllDispatcher);
    let config = AgentLoopConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        model: "test".to_string(),
        provider,
        hook_dispatcher: dispatcher,
        tools: vec![],
        prompt_builder: PromptBuilder::from("You are helpful."),
        stream_options: StreamOptions::default(),
        steer_queue: Arc::new(Mutex::new(vec![])),
        follow_up_queue: Arc::new(Mutex::new(vec![])),
        event_sink: Arc::new(move |event| {
            events_clone.lock().unwrap().push(event);
        }),
        circuit_breaker: None,
        skills: Vec::new(),
    };
    let loop_ = AgentLoop::new(config);

    let user_msg = ai_provider::Message::User(ai_provider::UserMessage {
        content: vec![Content::Text {
            text: "hi".to_string(),
            text_signature: None,
        }],
        timestamp: std::time::SystemTime::now(),
    });

    loop_
        .run(vec![user_msg], CancellationToken::new())
        .await
        .unwrap();

    let evs = events.lock().unwrap();
    let variant_names: Vec<&str> = evs
        .iter()
        .map(|e| match e {
            AgentEvent::AgentStart => "AgentStart",
            AgentEvent::TurnStart { .. } => "TurnStart",
            AgentEvent::MessageStart { .. } => "MessageStart",
            AgentEvent::MessageEnd { .. } => "MessageEnd",
            AgentEvent::TurnEnd { .. } => "TurnEnd",
            AgentEvent::AgentEnd { .. } => "AgentEnd",
            AgentEvent::AutoRetryEnd { success: true, .. } => "AutoRetryEnd",
            _ => "Other",
        })
        .collect();

    assert!(
        variant_names
            .iter()
            .position(|&n| n == "AgentStart")
            .is_some()
    );
    assert!(
        variant_names
            .iter()
            .position(|&n| n == "TurnStart")
            .is_some()
    );
    assert!(
        variant_names
            .iter()
            .position(|&n| n == "MessageStart")
            .is_some()
    );
    assert!(
        variant_names
            .iter()
            .position(|&n| n == "MessageEnd")
            .is_some()
    );
    assert!(variant_names.iter().position(|&n| n == "TurnEnd").is_some());
    assert!(
        variant_names
            .iter()
            .position(|&n| n == "AgentEnd")
            .is_some()
    );

    let agent_start_pos = variant_names
        .iter()
        .position(|&n| n == "AgentStart")
        .unwrap();
    let turn_start_pos = variant_names
        .iter()
        .position(|&n| n == "TurnStart")
        .unwrap();
    let msg_start_pos = variant_names
        .iter()
        .position(|&n| n == "MessageStart")
        .unwrap();
    let msg_end_pos = variant_names
        .iter()
        .position(|&n| n == "MessageEnd")
        .unwrap();
    let turn_end_pos = variant_names.iter().position(|&n| n == "TurnEnd").unwrap();
    let agent_end_pos = variant_names.iter().position(|&n| n == "AgentEnd").unwrap();

    assert!(agent_start_pos < turn_start_pos);
    assert!(turn_start_pos < msg_start_pos);
    assert!(msg_start_pos < msg_end_pos);
    assert!(msg_end_pos < turn_end_pos);
    assert!(turn_end_pos < agent_end_pos);
}

#[test]
fn test_resolve_orphan_injects_synthetic_error() {
    let mut messages = vec![
        ai_provider::Message::Assistant(ai_provider::AssistantMessage {
            content: vec![Content::ToolCall(ToolCall {
                id: "call_1".to_string(),
                name: "tool_a".to_string(),
                arguments: serde_json::json!({}),
                thought_signature: None,
            })],
            provider: "test".to_string(),
            model: "test".to_string(),
            api: ai_provider::Api {
                provider: "test".to_string(),
                model: "test".to_string(),
            },
            usage: ai_provider::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 0,
            },
            stop_reason: StopReason::ToolUse,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        }),
        ai_provider::Message::ToolResult(ai_provider::ToolResultMessage {
            tool_call_id: "call_2".to_string(),
            tool_name: "tool_b".to_string(),
            content: vec![],
            details: None,
            is_error: false,
            timestamp: std::time::SystemTime::now(),
        }),
    ];

    resolve_orphan_tool_calls(&mut messages);

    assert_eq!(messages.len(), 3);
    match &messages[1] {
        ai_provider::Message::ToolResult(tr) => {
            assert_eq!(tr.tool_call_id, "call_1");
            assert!(tr.is_error);
            assert!(tr.details.as_ref().unwrap()["_orphan"].as_bool().unwrap());
        }
        _ => panic!("expected orphan tool result"),
    }
}

#[tokio::test]
async fn test_complete_returns_text_only() {
    let _ = tracing_subscriber::fmt().try_init();

    let provider = TestProvider::text("hello world");
    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(SessionConfig {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "test".to_string(),
        provider: provider.clone(),
        hook_dispatcher: dispatcher,
        compaction_actor: Arc::new(agent_core::harness::compaction::Compactor::new(
            agent_core::harness::compaction::CompactionConfig::default(),
            provider,
            "test".to_string(),
            Arc::new(agent_core::DefaultFileOperationExtractor::default()),
        )),
        tools: vec![],
        store: None,
        skills: vec![],
    });

    let result: String = session.complete("hello".to_string()).await.unwrap();
    assert_eq!(result, "hello world");
}
