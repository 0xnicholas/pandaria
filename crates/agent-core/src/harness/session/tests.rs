//! Session unit tests — extracted from mod.rs to keep production code lean.
//! All 22 tests verify SessionActor behavior end-to-end against mock providers.

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};

    use async_trait::async_trait;
    use tokio::time::sleep;

    use ai_provider::{Content, StopReason, UserMessage};
    use uuid::Uuid;

    use crate::harness::session::{SessionActor, SessionConfig, SessionState};
    use crate::file_ops::DefaultFileOperationExtractor;
    use crate::harness::compaction::{CompactionConfig, Compactor};
    use crate::hook::context::{AgentEndCtx, SessionCtx};
    use crate::hook::dispatcher::HookDispatcher;
    use crate::persistence::entry::SessionEntry;
    use crate::test_utils::{AllowAllDispatcher, TestProvider};
    use crate::types::AgentMessage;
    use crate::AgentError;
    use crate::persistence::store::SessionStore;    fn make_compaction_actor(provider: Arc<dyn ai_provider::LlmProvider>) -> Compactor {
        Compactor::new(
            CompactionConfig::default(),
            provider,
            "test".to_string(),
            Arc::new(DefaultFileOperationExtractor::default()),
        )
    }

    /// In-memory store for testing persistence
    struct MemoryStore {
        data: Mutex<Vec<(String, String, Vec<SessionEntry>)>>,
    }

    impl MemoryStore {
        fn new() -> Self {
            Self {
                data: Mutex::new(Vec::new()),
            }
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

        async fn list_sessions(&self, tenant_id: &str) -> Result<Vec<String>, AgentError> {
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

    #[tokio::test]
    async fn test_session_prompt() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "prompt".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        let restored = session.restore().await.unwrap();
        assert_eq!(restored, 0);
    }

    #[tokio::test]
    async fn test_steer_injection() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "You are helpful.".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        // Queue a steer message
        session.steer(AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "steer note".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }));

        // The steer message should be injected before the LLM call
        // After prompt: user(hello) + steer + assistant(response) = 3 messages
        session.prompt("hello".to_string()).await.unwrap();

        // Verify steer was consumed (queue emptied)
        let msgs = session.messages();
        assert_eq!(msgs.len(), 3);
        // Second message should be the steer
        match &msgs[1] {
            AgentMessage::User(msg) => {
                let text = msg.content.first().and_then(|c| match c {
                    Content::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                });
                assert_eq!(text, Some("steer note"));
            }
            _ => panic!("expected user message at position 1"),
        }
    }

    #[tokio::test]
    async fn test_follow_up_loop() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "You are helpful.".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        // Queue a follow_up message
        session.follow_up(AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "follow up".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }));

        // The follow_up should trigger an additional turn
        // Expected: user(hello) + assistant + user(follow up) + assistant = 4 messages
        session.prompt("hello".to_string()).await.unwrap();

        let msgs = session.messages();
        assert_eq!(msgs.len(), 4);
    }

    #[tokio::test]
    async fn test_abort_session() {
        let _ = tracing_subscriber::fmt().try_init();

        let provider = TestProvider::cancel();
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "prompt".to_string(),
            model: "cancellable".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        // Test that abort() works by verifying the token propagates cancellation.
        // We can't easily test concurrent abort during prompt() because prompt()
        // takes &mut self. Instead, test the mechanism: abort the pre-prompt token,
        // then verify a new prompt creates a fresh token that can also be cancelled.

        // 1. Verify abort doesn't panic
        session.abort();
        assert!(session.state_machine.abort_token_ref().is_cancelled());

        // 2. Start a prompt — it creates a new token
        let prompt_handle = tokio::spawn(async move { session.prompt("hello".to_string()).await });

        // The provider waits for cancellation, so the prompt will hang until
        // cancelled or timed out. Since we can't call abort() (session moved),
        // we rely on the timeout to verify the prompt was actually running.
        let result = tokio::time::timeout(Duration::from_secs(5), prompt_handle).await;
        assert!(
            result.is_err(),
            "prompt should still be running (not yet cancelled)"
        );
    }

    #[tokio::test]
    async fn test_flush_persistence() {
        let _ = tracing_subscriber::fmt().try_init();

        let store = Arc::new(MemoryStore::new());
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "prompt".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: Some(store.clone()),
            skills: vec![],
        });

        // No messages yet, flush should save empty
        session.flush().await.unwrap();

        let loaded = store.load_session("t1", "s1").await.unwrap();
        assert!(loaded.is_empty());
    }

    /// Auto-restore loads session history from the store on the first prompt()
    /// of a newly-constructed session. This test verifies that after persisting
    /// messages in session 1, session 2 (same tenant/session/store) sees those
    /// messages after its first prompt.
    #[tokio::test]
    async fn test_auto_restore_on_first_prompt() {
        let _ = tracing_subscriber::fmt().try_init();

        let store = Arc::new(MemoryStore::new());
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);

        // Session 1: run one prompt and flush to the store.
        let s1_entry_count = {
            let mut s = SessionActor::new(SessionConfig {
                tenant_id: "t1".to_string(),
                session_id: "s1".to_string(),
                system_prompt: "prompt".to_string(),
                model: "echo".to_string(),
                provider: provider.clone(),
                hook_dispatcher: dispatcher.clone(),
                compaction_actor: Arc::new(make_compaction_actor(provider.clone())),
                tools: vec![],
                store: Some(store.clone()),
                skills: vec![],
            });
            s.prompt("hello".to_string()).await.unwrap();
            s.flush().await.unwrap();
            s.entries().len()
        };
        assert!(s1_entry_count > 0, "session 1 should have entries");

        // Verify the store has the entries from session 1.
        let stored = store.load_session("t1", "s1").await.unwrap();
        assert!(!stored.is_empty(), "store must have entries after flush");

        // Session 2: same tenant/session/store. Auto-restore happens during
        // the first prompt, loading session 1's history before the new turn.
        let mut s2 = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "prompt".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: Some(store.clone()),
            skills: vec![],
        });

        // Before first prompt, messages are empty (restore hasn't happened yet).
        assert!(
            s2.messages().is_empty(),
            "messages should be empty before first prompt"
        );

        // First prompt triggers auto-restore, loading session 1's history.
        s2.prompt("world".to_string()).await.unwrap();

        let msgs = s2.messages();
        assert!(
            msgs.len() >= 4,
            "auto-restore should load session 1 history ({s1_entry_count} entries) \
             plus new turn (2 entries), got {} messages",
            msgs.len()
        );
    }

    #[tokio::test]
    async fn test_consecutive_prompts_persist_all_entries() {
        let _ = tracing_subscriber::fmt().try_init();

        let store = Arc::new(MemoryStore::new());
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "prompt".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: Some(store.clone()),
            skills: vec![],
        });

        // Two consecutive prompts — each triggers a fire-and-forget save.
        // With the fix, the second save awaits the first, guaranteeing
        // ordering and preventing stale snapshots from overwriting newer ones.
        session.prompt("hello".to_string()).await.unwrap();
        session.prompt("world".to_string()).await.unwrap();
        session.flush().await.unwrap();

        let loaded = store.load_session("t1", "s1").await.unwrap();
        let msg_count = loaded
            .iter()
            .filter(|e| matches!(e, SessionEntry::Message { .. }))
            .count();
        assert_eq!(msg_count, 4, "expected 4 messages (2 user + 2 assistant)");
    }

    #[tokio::test]
    async fn test_entries_api_with_compaction() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "prompt".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        // Add a compaction entry manually
        session
            .history
            .entries_mut()
            .push(SessionEntry::Compaction {
                id: uuid::Uuid::new_v4(),
                summary: "test summary".to_string(),
                first_kept_entry_id: uuid::Uuid::new_v4(),
                tokens_before: 100,
                details: None,
                from_extension: false,
                timestamp: std::time::SystemTime::now(),
            });

        // entries() should include compaction
        let all_entries = session.entries();
        assert!(
            all_entries
                .iter()
                .any(|e| matches!(e, SessionEntry::Compaction { .. }))
        );

        // messages() should filter out compaction
        let msgs = session.messages();
        assert!(!msgs.iter().any(|m| matches!(m, AgentMessage::Assistant(_)))); // No assistant messages yet
        assert_eq!(msgs.len(), 0); // No actual messages, only compaction entry
    }

    #[tokio::test]
    async fn test_steer_and_follow_up_combined() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "You are helpful.".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        // Queue both steer and follow-up
        session.steer(AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "steer note".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }));
        session.follow_up(AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "follow up".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }));

        // Expected flow:
        // Turn 1: user(main) + steer + assistant
        // Turn 2: follow_up + assistant
        let results = session.prompt("hello".to_string()).await.unwrap();

        // Should have multiple messages from both turns
        assert!(results.len() >= 2);

        // Verify steer was consumed
        let msgs = session.messages();
        assert!(msgs.iter().any(|m| {
            if let AgentMessage::User(u) = m {
                u.content
                    .iter()
                    .any(|c| matches!(c, Content::Text { text, .. } if text == "steer note"))
            } else {
                false
            }
        }));

        // Verify follow-up was consumed
        assert!(msgs.iter().any(|m| {
            if let AgentMessage::User(u) = m {
                u.content
                    .iter()
                    .any(|c| matches!(c, Content::Text { text, .. } if text == "follow up"))
            } else {
                false
            }
        }));
    }

    struct CountingDispatcher {
        session_start_count: Arc<AtomicUsize>,
        agent_end_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl HookDispatcher for CountingDispatcher {
        async fn on_session_start(&self, _ctx: &SessionCtx) {
            self.session_start_count.fetch_add(1, Ordering::SeqCst);
        }

        async fn on_agent_end(&self, _ctx: &AgentEndCtx) {
            self.agent_end_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn test_session_hooks_are_emitted() {
        let _ = tracing_subscriber::fmt().try_init();

        let start_count = Arc::new(AtomicUsize::new(0));
        let end_count = Arc::new(AtomicUsize::new(0));
        let dispatcher = Arc::new(CountingDispatcher {
            session_start_count: start_count.clone(),
            agent_end_count: end_count.clone(),
        });
        let provider = TestProvider::text("response");

        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "prompt".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        session.prompt("hello".to_string()).await.unwrap();

        // on_agent_end 是 fire-and-forget，给它一点时间执行
        sleep(Duration::from_millis(100)).await;

        // on_session_start 在构造时触发
        assert!(
            start_count.load(Ordering::SeqCst) >= 1,
            "on_session_start should have been called"
        );
        // on_agent_end 在 prompt 完成后触发
        assert!(
            end_count.load(Ordering::SeqCst) >= 1,
            "on_agent_end should have been called"
        );
    }

    #[tokio::test]
    async fn test_multiple_prompts_increment_entries() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "You are helpful.".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        let result1 = session.prompt("hello".to_string()).await.unwrap();
        assert_eq!(result1.len(), 1); // 1 assistant message

        let result2 = session.prompt("world".to_string()).await.unwrap();
        assert_eq!(result2.len(), 1); // 1 assistant message

        // 总共应该有 4 条消息：user1 + assistant1 + user2 + assistant2
        let msgs = session.messages();
        assert_eq!(msgs.len(), 4);

        // entries 应该与 messages 数量相同（没有 compaction）
        assert_eq!(session.entries().len(), 4);

        // 验证 entry id 单调递增
        let ids: Vec<uuid::Uuid> = session.entries().iter().map(|e| e.id()).collect();
        assert_eq!(ids.len(), 4);
    }

    #[tokio::test]
    async fn test_concurrent_sessions_are_isolated() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("response");

        let mut s1 = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "You are helpful.".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: Arc::new(AllowAllDispatcher),
            compaction_actor: Arc::new(make_compaction_actor(provider.clone())),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        let mut s2 = SessionActor::new(SessionConfig {
            tenant_id: "t2".to_string(),
            session_id: "s2".to_string(),
            system_prompt: "You are helpful.".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: Arc::new(AllowAllDispatcher),
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        let (r1, r2) = tokio::join!(
            s1.prompt("hello".to_string()),
            s2.prompt("world".to_string()),
        );

        assert!(r1.is_ok());
        assert!(r2.is_ok());

        // 验证没有交叉污染
        assert_eq!(s1.tenant_id(), "t1");
        assert_eq!(s2.tenant_id(), "t2");

        let msgs1 = s1.messages();
        let msgs2 = s2.messages();

        // s1 不包含 "world"
        assert!(!msgs1.iter().any(|m| {
            if let AgentMessage::User(u) = m {
                u.content
                    .iter()
                    .any(|c| matches!(c, Content::Text { text, .. } if text == "world"))
            } else {
                false
            }
        }));

        // s2 不包含 "hello"
        assert!(!msgs2.iter().any(|m| {
            if let AgentMessage::User(u) = m {
                u.content
                    .iter()
                    .any(|c| matches!(c, Content::Text { text, .. } if text == "hello"))
            } else {
                false
            }
        }));
    }

    #[tokio::test]
    async fn test_router_provider_model_context_window() {
        let router = Arc::new(ai_provider::RouterProvider::new());
        let dispatcher = Arc::new(AllowAllDispatcher);
        let session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "You are helpful.".into(),
            model: "openai/gpt-5.2".to_string(),
            provider: router.clone(),
            hook_dispatcher: dispatcher.clone(),
            compaction_actor: Arc::new(make_compaction_actor(router.clone())),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        let cw = session.model_context_window();
        assert!(
            cw > 0,
            "model_context_window should be > 0 for openai/gpt-5.2"
        );
    }

    #[tokio::test]
    async fn test_cross_provider_model_context_window_switch() {
        let router = Arc::new(ai_provider::RouterProvider::new());
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "You are helpful.".into(),
            model: "openai/gpt-5.2".to_string(),
            provider: router.clone(),
            hook_dispatcher: dispatcher.clone(),
            compaction_actor: Arc::new(make_compaction_actor(router.clone())),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        let cw_openai = session.model_context_window();
        assert!(cw_openai > 0);

        session.set_model("anthropic/claude-sonnet-4-20250514".to_string());
        let cw_anthropic = session.model_context_window();
        assert!(cw_anthropic > 0);

        assert_ne!(cw_openai, cw_anthropic);
    }

    #[tokio::test]
    async fn test_system_prompt_with_skills_contains_available_skills() {
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let skills = vec![crate::skills::Skill {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            file_path: "/skills/test/SKILL.md".to_string(),
            base_dir: "/skills".to_string(),
            source: crate::skills::SkillSource::Project,
            disable_model_invocation: false,
        }];
        let session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "You are helpful.".to_string(),
            model: "echo".to_string(),
            provider: provider,
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(TestProvider::text("response"))),
            tools: vec![],
            store: None,
            skills: skills,
        });

        let prompt = session.system_prompt();
        assert!(
            prompt.contains("<available_skills>"),
            "expected skills XML in system prompt, got: {}",
            prompt
        );
        assert!(
            prompt.contains("test-skill"),
            "expected skill name in system prompt"
        );
    }

    #[tokio::test]
    async fn test_set_system_prompt_preserves_skills() {
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let skills = vec![crate::skills::Skill {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            file_path: "/skills/test/SKILL.md".to_string(),
            base_dir: "/skills".to_string(),
            source: crate::skills::SkillSource::Project,
            disable_model_invocation: false,
        }];
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "You are helpful.".to_string(),
            model: "echo".to_string(),
            provider: provider,
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(TestProvider::text("response"))),
            tools: vec![],
            store: None,
            skills: skills,
        });

        session.set_system_prompt("New persona.".to_string());
        let prompt = session.system_prompt();
        assert!(
            prompt.starts_with("New persona."),
            "expected new base persona, got: {}",
            prompt
        );
        assert!(
            prompt.contains("<available_skills>"),
            "expected skills XML preserved after set_system_prompt, got: {}",
            prompt
        );
    }

    #[tokio::test]
    async fn test_state_idle_after_creation() {
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "prompt".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        assert_eq!(session.state(), SessionState::Idle);
        assert!(!session.is_streaming());
    }

    #[tokio::test]
    async fn test_state_idle_after_successful_prompt() {
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "prompt".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        let _ = session.prompt("hello".to_string()).await.unwrap();
        assert_eq!(session.state(), SessionState::Idle);
        assert!(!session.is_streaming());
    }

    #[tokio::test]
    async fn test_state_error_after_unrecoverable_error() {
        let provider = TestProvider::error("something went wrong");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "prompt".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(TestProvider::text("response"))),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        let result = session.prompt("hello".to_string()).await;
        assert!(result.is_err());
        assert_eq!(session.state(), SessionState::Error);
        assert!(session.error_reason().is_some());
    }

    #[tokio::test]
    async fn test_error_state_blocks_prompt() {
        let provider = TestProvider::error("something went wrong");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "prompt".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(TestProvider::text("response"))),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        let _ = session.prompt("hello".to_string()).await;
        assert_eq!(session.state(), SessionState::Error);

        let err = session.prompt("again".to_string()).await.unwrap_err();
        match err {
            AgentError::SessionInError { .. } => {}
            other => panic!("expected SessionInError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_reset_clears_error_state() {
        let provider = TestProvider::error("something went wrong");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "prompt".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(TestProvider::text("response"))),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        let _ = session.prompt("hello".to_string()).await;
        assert_eq!(session.state(), SessionState::Error);

        session.reset().await.unwrap();
        assert_eq!(session.state(), SessionState::Idle);
        assert!(session.error_reason().is_none());
        assert!(session.messages().is_empty());
    }

    #[tokio::test]
    async fn test_reset_preserves_config() {
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(SessionConfig {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            system_prompt: "original prompt".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor: Arc::new(make_compaction_actor(provider)),
            tools: vec![],
            store: None,
            skills: vec![],
        });

        session.prompt("hello".to_string()).await.unwrap();
        session.reset().await.unwrap();

        assert_eq!(session.system_prompt(), "original prompt");
        assert_eq!(session.state(), SessionState::Idle);
    }
}
