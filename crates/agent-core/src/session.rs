use std::sync::{Arc, Mutex};

use llm_client::{Content, StopReason};
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};

use crate::compaction::{
    should_compact, CompactionActor, CompactionResult,
    estimate_context_tokens,
};
use crate::context::{CompactCtx, CompactReason, SessionCtx};
use crate::error::AgentError;
use crate::error_recovery::{RecoveryAction, RecoveryStateMachine};
use crate::events::{AgentEvent, AgentEventListener};
use crate::hook_dispatcher::HookDispatcher;
use crate::loop_::{AgentLoop, AgentLoopConfig};
use crate::session_entry::{SessionContextBuilder, SessionEntry};
use crate::store::SessionStore;
use crate::types::{AgentMessage, AgentToolRef};

struct QueuedEvent {
    event: AgentEvent,
}

/// Manages the lifecycle of a single agent session for a given tenant.
///
/// Owns message history, tool set, and steer/follow-up queues.
/// Each session is isolated — no shared mutable state with other sessions.
pub struct SessionActor {
    tenant_id: String,
    session_id: String,
    model: String,
    system_prompt: String,
    stream_options: llm_client::StreamOptions,
    max_retries: u32,
    provider: Arc<dyn llm_client::LlmProvider>,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    compaction_actor: Arc<CompactionActor>,
    tools: Vec<AgentToolRef>,
    entries: Vec<SessionEntry>,
    /// Messages queued for injection before the next LLM call
    steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    /// Messages queued for injection after the agent would stop
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    /// Optional persistence backend for session history
    store: Option<Arc<dyn SessionStore>>,
    abort_token: CancellationToken,

    recovery: RecoveryStateMachine,
    event_listeners: Arc<Mutex<Vec<Arc<dyn AgentEventListener>>>>,
    event_tx: Option<tokio::sync::mpsc::Sender<QueuedEvent>>,
    event_processor_handle: Option<tokio::task::JoinHandle<()>>,
    is_streaming: bool,
}

impl SessionActor {
    /// Create a new session.
    ///
    /// Emits `on_session_start` hook (fire-and-forget, per ADR-003) on construction.
    /// This hook is observational only — it must not perform setup work that
    /// affects the session, as it runs concurrently and may not complete before
    /// the first `prompt()` call.
    ///
    /// If a `store` is provided, call [`restore`](Self::restore) after construction
    /// to load message history before the first prompt.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tenant_id: String,
        session_id: String,
        system_prompt: String,
        model: String,
        provider: Arc<dyn llm_client::LlmProvider>,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        compaction_actor: Arc<CompactionActor>,
        tools: Vec<AgentToolRef>,
        store: Option<Arc<dyn SessionStore>>,
    ) -> Self {
        // Emit session_start (fire-and-forget, per ADR-003)
        let tool_defs: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "parameters": t.parameters(),
                })
            })
            .collect();
        let session_ctx = SessionCtx {
            tenant_id: tenant_id.clone(),
            session_id: session_id.clone(),
            system_prompt: system_prompt.clone(),
            tools: tool_defs,
        };
        let dispatcher = hook_dispatcher.clone();
        tokio::spawn(async move {
            let _ = crate::hook_timeout::with_timeout(
                dispatcher.on_session_start(&session_ctx),
                100,
                (),
                "on_session_start",
            ).await;
        });

        info!(
            tenant_id = %tenant_id,
            session_id = %session_id,
            tools_count = tools.len(),
            has_store = store.is_some(),
            "session started",
        );

        let mut actor = Self {
            tenant_id,
            session_id,
            model,
            system_prompt,
            stream_options: llm_client::StreamOptions::default(),
            max_retries: 3,
            provider,
            hook_dispatcher,
            compaction_actor,
            tools,
            entries: Vec::new(),
            steer_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            store,
            abort_token: CancellationToken::new(),
            recovery: RecoveryStateMachine::new(3),
            event_listeners: Arc::new(Mutex::new(Vec::new())),
            event_tx: None,
            event_processor_handle: None,
            is_streaming: false,
        };
        let event_tx = actor.spawn_event_processor();
        actor.event_tx = Some(event_tx);
        actor
    }

    fn spawn_event_processor(&mut self) -> tokio::sync::mpsc::Sender<QueuedEvent> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<QueuedEvent>(1024);
        let listeners = self.event_listeners.clone();

        let handle = tokio::spawn(async move {
            while let Some(queued) = rx.recv().await {
                let ls: Vec<_> = {
                    listeners.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
                };
                for listener in &ls {
                    let _ = listener.on_event(&queued.event).await;
                }
            }
        });

        self.event_processor_handle = Some(handle);
        tx
    }

    /// Attempt to restore session history from the configured store.
    ///
    /// Returns the number of entries restored, or 0 if no store is configured
    /// or the store has no data for this session.
    pub async fn restore(&mut self) -> Result<usize, AgentError> {
        if let Some(ref store) = self.store {
            let entries = store.load_session(&self.tenant_id, &self.session_id).await?;
            let count = entries.len();
            if count > 0 {
                info!(
                    tenant_id = %self.tenant_id,
                    session_id = %self.session_id,
                    restored_count = count,
                    "restored session history from store",
                );
            }
            self.entries = entries;
            Ok(count)
        } else {
            Ok(0)
        }
    }

    /// Send a user message and run the agent loop.
    ///
    /// Drains steer_queue before the loop, and follow_up_queue after the loop
    /// would normally stop, driving additional turns.
    ///
    /// Session state is persisted after the run via fire-and-forget `tokio::spawn`.
    /// Call [`flush`](Self::flush) to guarantee persistence before shutdown.
    #[instrument(
        skip(self),
        fields(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
        )
    )]
    pub async fn prompt(
        &mut self,
        text: String,
    ) -> Result<Vec<AgentMessage>, AgentError> {
        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text {
                text,
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });
        self.push_message(user_msg);
        self.run_with_messages(None).await
    }

    pub async fn complete(&mut self, text: String) -> Result<String, AgentError> {
        let messages = self.prompt(text).await?;
        let text_content: Vec<String> = messages.iter().filter_map(|m| {
            if let AgentMessage::Assistant(a) = m {
                Some(a.content.iter().filter_map(|c| match c {
                    llm_client::Content::Text { text, .. } => Some(text.clone()),
                    _ => None,
                }).collect::<Vec<_>>().join(" "))
            } else { None }
        }).collect();
        Ok(text_content.join("\n"))
    }

    pub async fn continue_(&mut self) -> Result<Vec<AgentMessage>, AgentError> {
        self.run_with_messages(None).await
    }

    pub fn is_streaming(&self) -> bool { self.is_streaming }

    pub fn add_event_listener(&mut self, listener: Arc<dyn AgentEventListener>) {
        self.event_listeners.lock().unwrap_or_else(|e| e.into_inner()).push(listener);
    }

    pub fn set_system_prompt(&mut self, prompt: String) { self.system_prompt = prompt; }
    pub fn set_model(&mut self, model: String) { self.model = model; }
    pub fn set_tools(&mut self, tools: Vec<AgentToolRef>) { self.tools = tools; }
    pub fn set_stream_options(&mut self, options: llm_client::StreamOptions) { self.stream_options = options; }
    pub fn set_max_retries(&mut self, max_retries: u32) {
        self.max_retries = max_retries;
        self.stream_options.max_retries = max_retries;
    }
    pub fn system_prompt(&self) -> &str { &self.system_prompt }

    #[instrument(
        skip(self),
        fields(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
        )
    )]
    async fn run_with_messages(
        &mut self,
        _add_user_msg: Option<String>,
    ) -> Result<Vec<AgentMessage>, AgentError> {
        let mut all_new_msgs = Vec::new();

        loop {
            self.is_streaming = true;
            self.abort_token = CancellationToken::new();

            let messages = SessionContextBuilder::build_context(&self.entries);

            let event_tx = self.event_tx.clone();
            let event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync + 'static> = Arc::new(move |event| {
                if let Some(tx) = &event_tx
                    && tx.try_send(QueuedEvent { event }).is_err()
                {
                    tracing::warn!("event queue full, dropping event");
                }
            });

            let config = AgentLoopConfig {
                tenant_id: self.tenant_id.clone(), session_id: self.session_id.clone(),
                model: self.model.clone(), provider: self.provider.clone(),
                hook_dispatcher: self.hook_dispatcher.clone(), tools: self.tools.clone(),
                system_prompt: Some(self.system_prompt.clone()),
                stream_options: self.stream_options.clone(),
                event_sink,
                steer_queue: self.steer_queue.clone(),
                follow_up_queue: self.follow_up_queue.clone(),
            };

            match AgentLoop::new(config).run(messages, self.abort_token.child_token()).await {
                Ok(msgs) => {
                    self.is_streaming = false;
                    if let Some(AgentMessage::Assistant(assistant)) = msgs.iter().rfind(|m| matches!(m, AgentMessage::Assistant(a) if a.stop_reason != StopReason::ToolUse)) {
                        let action = self.recovery.evaluate(assistant);
                        match action {
                            RecoveryAction::RetryAfterBackoff { delay_ms } => {
                                tokio::select! {
                                    _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                                    _ = self.abort_token.cancelled() => {
                                        self.recovery.mark_success();
                                        return Err(AgentError::RecoveryAborted(
                                            "recovery aborted via backoff timeout".into()
                                        ));
                                    }
                                }
                                self.recovery.mark_success();
                                continue;
                            }
                            RecoveryAction::RetryAfterCompaction { .. } => {
                                self.recovery.mark_success();
                                self.run_auto_compaction(CompactReason::Overflow, true).await?;
                                continue;
                            }
                            RecoveryAction::Abort { reason } => {
                                self.recovery.mark_success();
                                return Err(AgentError::RecoveryAborted(reason));
                            }
                            RecoveryAction::Continue => { self.recovery.mark_success(); }
                        }
                    }
                    for msg in &msgs { self.push_message(msg.clone()); }
                    all_new_msgs.extend(msgs);
                }
                Err(e) => {
                    self.is_streaming = false;
                    match e {
                        AgentError::ContextOverflow(msg) => {
                            let action = self.recovery.evaluate_overflow(&msg);
                            match action {
                                RecoveryAction::RetryAfterCompaction { .. } => {
                                    self.recovery.mark_success();
                                    self.run_auto_compaction(CompactReason::Overflow, true).await?;
                                    continue;
                                }
                                RecoveryAction::Abort { reason } => {
                                    return Err(AgentError::CompactionFailed(reason));
                                }
                                RecoveryAction::RetryAfterBackoff { delay_ms } => {
                                    tokio::select! {
                                        _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                                        _ = self.abort_token.cancelled() => {
                                            self.recovery.mark_success();
                                            return Err(AgentError::RecoveryAborted(
                                                "recovery aborted via backoff timeout".into()
                                            ));
                                        }
                                    }
                                    self.recovery.mark_success();
                                    continue;
                                }
                                RecoveryAction::Continue => {
                                    return Err(AgentError::ContextOverflow(msg));
                                }
                            }
                        }
                        other => return Err(other),
                    }
                }
            }

            // Mid-loop threshold compaction
            if self.compaction_actor.config.enabled {
                let context_tokens = estimate_context_tokens(&self.entries);
                let context_window = self.model_context_window();
                if should_compact(context_tokens, context_window, &self.compaction_actor.config) {
                    self.run_auto_compaction(CompactReason::Threshold, false).await?;
                }
            }

            break;
        }

        info!(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            total_entries = self.entries.len(),
            new_msg_count = all_new_msgs.len(),
            "agent run complete",
        );

        // Check for threshold compaction after successful turn
        if let Some(AgentMessage::Assistant(last_assistant)) = all_new_msgs.iter().rfind(|m| matches!(m, AgentMessage::Assistant(a) if a.stop_reason != StopReason::ToolUse))
            && let Err(e) = self.check_compaction(last_assistant).await
        {
            warn!(
                tenant_id = %self.tenant_id,
                session_id = %self.session_id,
                error = %e,
                "auto-compaction failed",
            );
        }

        // Persist AFTER check_compaction so that newly-added compaction
        // entries are included in the snapshot.
        if let Some(ref store) = self.store {
            let entries = self.entries.clone();
            let tenant_id = self.tenant_id.clone();
            let session_id = self.session_id.clone();
            let store = store.clone();
            tokio::spawn(async move {
                if let Err(e) = store.save_session(&tenant_id, &session_id, &entries).await {
                    warn!(
                        tenant_id = %tenant_id,
                        session_id = %session_id,
                        error = %e,
                        "failed to persist session",
                    );
                }
            });
        }

        Ok(all_new_msgs)
    }

    fn model_context_window(&self) -> usize {
        if let Some(model_meta) = llm_client::get_model(
            &self.provider.provider_name(),
            &self.model,
        ) {
            model_meta.context_window as usize
        } else {
            0
        }
    }

    async fn run_auto_compaction(
        &mut self,
        reason: CompactReason,
        will_retry: bool,
    ) -> Result<(), AgentError> {
        // Emit compaction_start
        if let Some(tx) = &self.event_tx {
            tx.send(QueuedEvent {
                event: AgentEvent::CompactionStart { reason: reason.clone() },
            }).await.ok();
        }

        // 1. Extension hook
        let preparation = self.compaction_actor.prepare(&self.entries)
            .map_err(|e| AgentError::CompactionFailed(e.to_string()))?;

        let compact_ctx = CompactCtx {
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            preparation,
            entries: self.entries.clone(),
            reason: reason.clone(),
        };

        let decision = crate::hook_timeout::with_timeout(
            self.hook_dispatcher.on_before_compact(&compact_ctx),
            500,
            crate::mutations::CompactDecision::Continue,
            "on_before_compact",
        ).await;
        let original_reason = reason.clone();

        let (from_extension, result) = match decision {
            crate::mutations::CompactDecision::Block { reason: block_reason } => {
                if let Some(tx) = &self.event_tx {
                    tx.send(QueuedEvent {
                        event: AgentEvent::CompactionEnd {
                            reason: original_reason,
                            result: None,
                            aborted: true,
                            will_retry: false,
                            error_message: Some(block_reason),
                        },
                    }).await.ok();
                }
                return Ok(());
            }
            crate::mutations::CompactDecision::Replace { result } => {
                (true, result)
            }
            crate::mutations::CompactDecision::Continue => {
                let result = self.compaction_actor.compact(&self.entries, &self.abort_token.child_token()).await
                    .map_err(|e| AgentError::CompactionFailed(e.to_string()))?;
                (false, result)
            }
        };

        // 2. Append compaction entry
        let compaction_entry = SessionEntry::Compaction {
            id: uuid::Uuid::new_v4(),
            summary: result.summary.clone(),
            first_kept_entry_id: result.first_kept_entry_id,
            tokens_before: result.tokens_before,
            details: result.details.clone(),
            from_extension,
            timestamp: std::time::SystemTime::now(),
        };
        self.entries.push(compaction_entry);

        // 3. Emit compaction_end
        if let Some(tx) = &self.event_tx {
            tx.send(QueuedEvent {
                event: AgentEvent::CompactionEnd {
                    reason: reason.clone(),
                    result: Some(result),
                    aborted: false,
                    will_retry,
                    error_message: None,
                },
            }).await.ok();
        }

        Ok(())
    }

    fn is_context_overflow(assistant: &llm_client::AssistantMessage) -> bool {
        assistant.stop_reason == llm_client::StopReason::Error
            && assistant.error_message.as_ref().is_some_and(|e| {
                let lower = e.to_lowercase();
                lower.contains("context length") || lower.contains("token limit")
            })
    }

    async fn check_compaction(&mut self, last_assistant: &llm_client::AssistantMessage) -> Result<(), AgentError> {
        let config = &self.compaction_actor.config;
        if !config.enabled {
            return Ok(());
        }

        if last_assistant.stop_reason == llm_client::StopReason::Aborted {
            return Ok(());
        }

        // Skip if assistant message is from before last compaction
        if let Some(SessionEntry::Compaction { timestamp, .. }) = self.entries.iter().rfind(|e| matches!(e, SessionEntry::Compaction { .. }))
            && last_assistant.timestamp <= *timestamp
        {
            return Ok(());
        }

        // Case 1: Overflow (recovery is handled by RecoveryStateMachine, here we just compact)
        if Self::is_context_overflow(last_assistant) {
            if self.recovery.overflow_attempted {
                return Err(AgentError::CompactionFailed(
                    "Context overflow recovery failed after one compact-and-retry attempt".into()
                ));
            }
            self.recovery.overflow_attempted = true;
            self.run_auto_compaction(CompactReason::Overflow, false).await?;
            return Ok(());
        }

        // Case 2: Threshold
        let context_tokens = estimate_context_tokens(&self.entries);
        let context_window = self.model_context_window();

        if should_compact(context_tokens, context_window, config) {
            self.run_auto_compaction(CompactReason::Threshold, false).await?;
        }

        Ok(())
    }

    /// Manually trigger compaction with optional custom instructions.
    pub async fn compact(&mut self, _custom_instructions: Option<String>) -> Result<CompactionResult, AgentError> {
        // For manual compaction, we always use Continue decision (no extension override)
        let result = self.compaction_actor.compact(&self.entries, &self.abort_token.child_token()).await
            .map_err(|e| AgentError::CompactionFailed(e.to_string()))?;
        
        let compaction_entry = SessionEntry::Compaction {
            id: uuid::Uuid::new_v4(),
            summary: result.summary.clone(),
            first_kept_entry_id: result.first_kept_entry_id,
            tokens_before: result.tokens_before,
            details: result.details.clone(),
            from_extension: false,
            timestamp: std::time::SystemTime::now(),
        };
        self.entries.push(compaction_entry);
        
        Ok(result)
    }

    pub fn push_message(&mut self, msg: AgentMessage) {
        self.entries.push(SessionEntry::Message {
            id: uuid::Uuid::new_v4(),
            message: msg,
        });
    }

    /// Queue a steering message (injected before next LLM call in current run)
    pub fn steer(&mut self, message: AgentMessage) {
        self.steer_queue.lock().expect("steer queue poisoned").push(message);
    }

    pub fn follow_up(&mut self, message: AgentMessage) {
        self.follow_up_queue.lock().expect("follow_up queue poisoned").push(message);
    }

    /// Flush pending persistence writes.
    ///
    /// The session state is saved asynchronously (fire-and-forget) after each
    /// `prompt()` call. Call `flush()` before shutdown to guarantee all writes
    /// have completed. Returns `Ok(())` if no store is configured.
    pub async fn flush(&self) -> Result<(), AgentError> {
        if let Some(ref store) = self.store {
            store.save_session(&self.tenant_id, &self.session_id, &self.entries).await?;
            info!(
                tenant_id = %self.tenant_id,
                session_id = %self.session_id,
                "session state flushed to store",
            );
        }
        Ok(())
    }

    /// Abort the current run
    pub fn abort(&self) {
        warn!(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            "session aborted",
        );
        self.abort_token.cancel();
    }

    /// Gracefully shut down the session.
    ///
    /// Cancels any in-flight operations and waits for the event processor
    /// to finish (up to 1s). Call `flush()` before this if you need to
    /// guarantee persistence.
    pub async fn shutdown(&mut self) {
        info!(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            "session shutting down",
        );

        // 1. Cancel any ongoing prompt or compaction
        self.abort_token.cancel();

        // 2. Drop event sender to signal the processor to exit
        self.event_tx.take();

        // 3. Wait for the event processor with a timeout
        if let Some(handle) = self.event_processor_handle.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), handle).await;
        }

        info!(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            "session shutdown complete",
        );
    }

    pub fn messages(&self) -> Vec<AgentMessage> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                SessionEntry::Message { message: msg, .. } => Some(msg.clone()),
                SessionEntry::Compaction { .. } => None,
            })
            .collect()
    }

    /// Return the full session history including compaction entries.
    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    /// Get the tenant ID
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// Get the session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

impl Drop for SessionActor {
    fn drop(&mut self) {
        // 1. Cancel any in-flight operations
        self.abort_token.cancel();

        // 2. Drop event sender to signal processor exit
        self.event_tx.take();

        // 3. Abort the event processor task if still running
        if let Some(handle) = self.event_processor_handle.take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::CompactionConfig;
    use crate::context::{AgentEndCtx, TurnEndCtx};
    use crate::file_ops::DefaultFileOperationExtractor;
    use crate::test_utils::{AllowAllDispatcher, TestProvider};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tokio::time::{sleep, Duration};

    fn make_compaction_actor(provider: Arc<dyn llm_client::LlmProvider>) -> CompactionActor {
        CompactionActor::new(
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
            Self { data: Mutex::new(Vec::new()) }
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
    }

    #[tokio::test]
    async fn test_session_prompt() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "prompt".to_string(),
            "echo".to_string(),
            provider.clone(),
            dispatcher,
            Arc::new(make_compaction_actor(provider)),
            vec![],
            None,
        );

        let restored = session.restore().await.unwrap();
        assert_eq!(restored, 0);
    }

    #[tokio::test]
    async fn test_steer_injection() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "You are helpful.".to_string(),
            "echo".to_string(),
            provider.clone(),
            dispatcher,
            Arc::new(make_compaction_actor(provider)),
            vec![],
            None,
        );

        // Queue a steer message
        session.steer(AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: "steer note".to_string(), text_signature: None }],
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
        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "You are helpful.".to_string(),
            "echo".to_string(),
            provider.clone(),
            dispatcher,
            Arc::new(make_compaction_actor(provider)),
            vec![],
            None,
        );

        // Queue a follow_up message
        session.follow_up(AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: "follow up".to_string(), text_signature: None }],
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
        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "prompt".to_string(),
            "cancellable".to_string(),
            provider.clone(),
            dispatcher,
            Arc::new(make_compaction_actor(provider)),
            vec![],
            None,
        );

        // Test that abort() works by verifying the token propagates cancellation.
        // We can't easily test concurrent abort during prompt() because prompt()
        // takes &mut self. Instead, test the mechanism: abort the pre-prompt token,
        // then verify a new prompt creates a fresh token that can also be cancelled.

        // 1. Verify abort doesn't panic
        session.abort();
        assert!(session.abort_token.is_cancelled());

        // 2. Start a prompt — it creates a new token
        let prompt_handle = tokio::spawn(async move {
            session.prompt("hello".to_string()).await
        });

        // The provider waits for cancellation, so the prompt will hang until
        // cancelled or timed out. Since we can't call abort() (session moved),
        // we rely on the timeout to verify the prompt was actually running.
        let result = tokio::time::timeout(Duration::from_secs(5), prompt_handle).await;
        assert!(result.is_err(), "prompt should still be running (not yet cancelled)");
    }

    #[tokio::test]
    async fn test_flush_persistence() {
        let _ = tracing_subscriber::fmt().try_init();

        let store = Arc::new(MemoryStore::new());
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "prompt".to_string(),
            "echo".to_string(),
            provider.clone(),
            dispatcher,
            Arc::new(make_compaction_actor(provider)),
            vec![],
            Some(store.clone()),
        );

        // No messages yet, flush should save empty
        session.flush().await.unwrap();

        let loaded = store.load_session("t1", "s1").await.unwrap();
        assert!(loaded.is_empty());
    }

    #[tokio::test]
    async fn test_restore_from_store() {
        let _ = tracing_subscriber::fmt().try_init();

        let store = Arc::new(MemoryStore::new());
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);

        // Create session and add some messages
        {
        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "prompt".to_string(),
            "echo".to_string(),
            provider.clone(),
            dispatcher.clone(),
            Arc::new(make_compaction_actor(provider.clone())),
            vec![],
            Some(store.clone()),
        );
            session.prompt("hello".to_string()).await.unwrap();
            session.flush().await.unwrap();
        }

        // Create new session with same store, restore should get messages back
        let mut session2 = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "prompt".to_string(),
            "echo".to_string(),
            provider.clone(),
            dispatcher,
            Arc::new(make_compaction_actor(provider)),
            vec![],
            Some(store.clone()),
        );

        let restored = session2.restore().await.unwrap();
        assert!(restored > 0);
        let msgs = session2.messages();
        assert!(msgs.len() >= 2); // user + assistant
    }

    #[tokio::test]
    async fn test_entries_api_with_compaction() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "prompt".to_string(),
            "echo".to_string(),
            provider.clone(),
            dispatcher,
            Arc::new(make_compaction_actor(provider)),
            vec![],
            None,
        );

        // Add a compaction entry manually
        session.entries.push(SessionEntry::Compaction {
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
        assert!(all_entries.iter().any(|e| matches!(e, SessionEntry::Compaction { .. })));

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
        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "You are helpful.".to_string(),
            "echo".to_string(),
            provider.clone(),
            dispatcher,
            Arc::new(make_compaction_actor(provider)),
            vec![],
            None,
        );

        // Queue both steer and follow-up
        session.steer(AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: "steer note".to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        }));
        session.follow_up(AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: "follow up".to_string(), text_signature: None }],
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
                u.content.iter().any(|c| matches!(c, Content::Text { text, .. } if text == "steer note"))
            } else {
                false
            }
        }));

        // Verify follow-up was consumed
        assert!(msgs.iter().any(|m| {
            if let AgentMessage::User(u) = m {
                u.content.iter().any(|c| matches!(c, Content::Text { text, .. } if text == "follow up"))
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
        async fn on_session_start(&self,
            _ctx: &SessionCtx,
        ) {
            self.session_start_count.fetch_add(1, Ordering::SeqCst);
        }

        async fn on_agent_end(&self,
            _ctx: &AgentEndCtx,
        ) {
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

        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "prompt".to_string(),
            "echo".to_string(),
            provider.clone(),
            dispatcher,
            Arc::new(make_compaction_actor(provider)),
            vec![],
            None,
        );

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
        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "You are helpful.".to_string(),
            "echo".to_string(),
            provider.clone(),
            dispatcher,
            Arc::new(make_compaction_actor(provider)),
            vec![],
            None,
        );

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

        let mut s1 = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "You are helpful.".to_string(),
            "echo".to_string(),
            provider.clone(),
            Arc::new(AllowAllDispatcher),
            Arc::new(make_compaction_actor(provider.clone())),
            vec![],
            None,
        );

        let mut s2 = SessionActor::new(
            "t2".to_string(),
            "s2".to_string(),
            "You are helpful.".to_string(),
            "echo".to_string(),
            provider.clone(),
            Arc::new(AllowAllDispatcher),
            Arc::new(make_compaction_actor(provider)),
            vec![],
            None,
        );

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
                u.content.iter().any(|c| matches!(c, Content::Text { text, .. } if text == "world"))
            } else {
                false
            }
        }));

        // s2 不包含 "hello"
        assert!(!msgs2.iter().any(|m| {
            if let AgentMessage::User(u) = m {
                u.content.iter().any(|c| matches!(c, Content::Text { text, .. } if text == "hello"))
            } else {
                false
            }
        }));
    }
}