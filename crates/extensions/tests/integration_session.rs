use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use agent_core::context::{AgentEndCtx, SessionCtx, TurnEndCtx};
use agent_core::session::SessionActor;
use agent_core::store::SessionStore;
use agent_core::types::{AgentMessage, SessionEntry};
use agent_core::compaction::{CompactionActor, CompactionConfig};
use agent_core::error::AgentError;
use agent_core::file_ops::DefaultFileOperationExtractor;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;
use llm_client::Content;

// ============================================================================
// Mock LLM Provider
// ============================================================================

struct EchoProvider;

#[async_trait]
impl llm_client::LlmProvider for EchoProvider {
    fn provider_name(&self) -> &str { "echo" }
    fn models(&self) -> Vec<String> { vec!["echo".to_string()] }

    async fn stream(
        &self,
        _model: &str,
        _context: llm_client::LlmContext,
        _options: llm_client::StreamOptions,
        _signal: CancellationToken,
    ) -> Result<llm_client::AssistantMessageEventStream, llm_client::LlmError> {
        let (stream, tx) = llm_client::AssistantMessageEventStream::new(4);

        let partial = llm_client::AssistantMessage {
            content: vec![Content::Text { text: "response".to_string(), text_signature: None }],
            provider: "echo".to_string(),
            model: "echo".to_string(),
            api: llm_client::Api { provider: "echo".to_string(), model: "echo".to_string() },
            usage: llm_client::Usage {
                input_tokens: 0, output_tokens: 1,
                cache_creation_input_tokens: None, cache_read_input_tokens: None,
                total_tokens: 1,
            },
            stop_reason: llm_client::StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        };

        let events = vec![
            llm_client::AssistantMessageEvent::Start { partial: partial.clone() },
            llm_client::AssistantMessageEvent::Done { reason: llm_client::StopReason::Stop, message: partial },
        ];

        tokio::spawn(async move {
            for event in events {
                if tx.send(event).await.is_err() { break; }
            }
        });

        Ok(stream)
    }
}

fn make_compaction_actor(provider: Arc<dyn llm_client::LlmProvider>) -> Arc<CompactionActor> {
    Arc::new(CompactionActor::new(
        CompactionConfig::default(),
        provider,
        "echo".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    ))
}

// ============================================================================
// Mock Store
// ============================================================================

struct MemoryStore {
    data: std::sync::Mutex<Vec<(String, String, Vec<SessionEntry>)>>,
}

impl MemoryStore {
    fn new() -> Self {
        Self { data: std::sync::Mutex::new(Vec::new()) }
    }
}

#[async_trait]
impl SessionStore for MemoryStore {
    async fn save_session(
        &self,
        tenant_id: &str,
        session_id: &str,
        entries: &[SessionEntry],
    ) -> Result<(), AgentError> {
        self.data.lock().unwrap().push((
            tenant_id.to_string(),
            session_id.to_string(),
            entries.to_vec(),
        ));
        Ok(())
    }

    async fn load_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<Vec<SessionEntry>, AgentError> {
        let data = self.data.lock().unwrap();
        let msgs = data
            .iter()
            .rev()
            .find_map(|(tid, sid, msgs)| {
                if tid == tenant_id && sid == session_id {
                    Some(msgs.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        Ok(msgs)
    }

    async fn delete_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<(), AgentError> {
        let mut data = self.data.lock().unwrap();
        data.retain(|(tid, sid, _)| !(tid == tenant_id && sid == session_id));
        Ok(())
    }

    async fn list_sessions(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<String>, AgentError> {
        let data = self.data.lock().unwrap();
        let mut sids: Vec<String> = data
            .iter()
            .filter(|(tid, _, _)| tid == tenant_id)
            .map(|(_, sid, _)| sid.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        sids.sort();
        Ok(sids)
    }
}

// ============================================================================
// Mock Extensions
// ============================================================================

struct SessionLifecycleExt {
    session_start_count: AtomicUsize,
    turn_end_count: AtomicUsize,
    agent_end_count: AtomicUsize,
}

#[async_trait]
impl Extension for SessionLifecycleExt {
    fn name(&self) -> &str { "lifecycle" }

    async fn on_session_start(&self, _ctx: &SessionCtx) {
        self.session_start_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {
        self.turn_end_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {
        self.agent_end_count.fetch_add(1, Ordering::SeqCst);
    }
}

// ============================================================================
// Tests: SessionActor + HookRouter integration
// ============================================================================

#[tokio::test]
async fn test_session_prompt_with_router() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let router = HookRouter::new(vec![], bus);

    let provider = Arc::new(EchoProvider);
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".to_string(),
        "echo".to_string(),
        provider,
        Arc::new(router),
        compaction_actor,
        vec![],
        None,
    );

    let results = session.prompt("hello".to_string()).await.unwrap();
    assert!(!results.is_empty());

    let msgs = session.messages();
    assert!(msgs.len() >= 2); // user + assistant
}

#[tokio::test]
async fn test_session_observational_hooks() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(SessionLifecycleExt {
        session_start_count: AtomicUsize::new(0),
        turn_end_count: AtomicUsize::new(0),
        agent_end_count: AtomicUsize::new(0),
    });
    let (handle, _) = ExtensionActor::spawn(ext.clone(), bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);

    let provider = Arc::new(EchoProvider);
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".to_string(),
        "echo".to_string(),
        provider,
        Arc::new(router),
        compaction_actor,
        vec![],
        None,
    );

    // session_start was fired on construction
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(ext.session_start_count.load(Ordering::SeqCst), 1);

    // prompt should trigger turn_end and agent_end
    session.prompt("hello".to_string()).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(ext.turn_end_count.load(Ordering::SeqCst), 1);
    assert_eq!(ext.agent_end_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_session_persistence_with_router() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let router = HookRouter::new(vec![], bus);
    let store = Arc::new(MemoryStore::new());

    let provider = Arc::new(EchoProvider);
    let compaction_actor = make_compaction_actor(provider.clone());

    // Create session, prompt, and flush
    {
        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "You are helpful.".to_string(),
            "echo".to_string(),
            provider.clone(),
            Arc::new(router),
            compaction_actor,
            vec![],
            Some(store.clone()),
        );

        session.prompt("hello".to_string()).await.unwrap();
        session.flush().await.unwrap();
    }

    // Restore into new session
    let bus2 = Arc::new(EventBus::<ObsEvent>::new(16));
    let router2 = HookRouter::new(vec![], bus2);
    let compaction_actor2 = make_compaction_actor(provider.clone());
    let mut session2 = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".to_string(),
        "echo".to_string(),
        provider,
        Arc::new(router2),
        compaction_actor2,
        vec![],
        Some(store.clone()),
    );

    let restored = session2.restore().await.unwrap();
    assert!(restored > 0);
    let msgs = session2.messages();
    assert!(msgs.len() >= 2);
}

#[tokio::test]
async fn test_session_steer_with_extension_hooks() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(SessionLifecycleExt {
        session_start_count: AtomicUsize::new(0),
        turn_end_count: AtomicUsize::new(0),
        agent_end_count: AtomicUsize::new(0),
    });
    let (handle, _) = ExtensionActor::spawn(ext.clone(), bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);

    let provider = Arc::new(EchoProvider);
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".to_string(),
        "echo".to_string(),
        provider,
        Arc::new(router),
        compaction_actor,
        vec![],
        None,
    );

    // Queue a steer message
    session.steer(AgentMessage::User(llm_client::UserMessage {
        content: vec![Content::Text { text: "steer note".to_string(), text_signature: None }],
        timestamp: std::time::SystemTime::now(),
    }));

    session.prompt("hello".to_string()).await.unwrap();

    let msgs = session.messages();
    assert!(msgs.len() >= 3); // user(main) + steer + assistant

    // Verify steer was consumed
    assert!(msgs.iter().any(|m| {
        if let AgentMessage::User(u) = m {
            u.content.iter().any(|c| matches!(c, Content::Text { text, .. } if text == "steer note"))
        } else {
            false
        }
    }));
}

#[tokio::test]
async fn test_session_follow_up_with_extension_hooks() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(SessionLifecycleExt {
        session_start_count: AtomicUsize::new(0),
        turn_end_count: AtomicUsize::new(0),
        agent_end_count: AtomicUsize::new(0),
    });
    let (handle, _) = ExtensionActor::spawn(ext.clone(), bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);

    let provider = Arc::new(EchoProvider);
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".to_string(),
        "echo".to_string(),
        provider,
        Arc::new(router),
        compaction_actor,
        vec![],
        None,
    );

    // Queue follow-up
    session.follow_up(AgentMessage::User(llm_client::UserMessage {
        content: vec![Content::Text { text: "follow up".to_string(), text_signature: None }],
        timestamp: std::time::SystemTime::now(),
    }));

    session.prompt("hello".to_string()).await.unwrap();

    // Should trigger 2 turns = 2 turn_end + 1 agent_end
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(ext.turn_end_count.load(Ordering::SeqCst), 2);
    assert_eq!(ext.agent_end_count.load(Ordering::SeqCst), 1);

    let msgs = session.messages();
    assert!(msgs.len() >= 4); // user + assistant + follow_up + assistant
}
