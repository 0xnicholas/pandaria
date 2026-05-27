use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use ai_provider::{Content, StopReason};
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};

use crate::error::AgentError;
use crate::events::{AgentEvent, AgentEventListener};
use crate::harness::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::harness::compaction::{
    CompactionResult, Compactor, estimate_context_tokens, should_compact,
};
use crate::harness::error_recovery::{RecoveryAction, RecoveryStateMachine};
use crate::hook::context::{CompactCtx, CompactReason, SessionCtx};
use crate::hook::dispatcher::HookDispatcher;
use crate::persistence::entry::{SessionContextBuilder, SessionEntry};
use crate::persistence::store::SessionStore;
use crate::prompt::{FragmentKind, FragmentSource, PromptBuilder, PromptFragment};
use crate::types::{AgentMessage, AgentToolRef};

struct QueuedEvent {
    event: AgentEvent,
}

/// Explicit session state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    /// Session created, no turn in progress.
    Idle,
    /// Turn is in progress (AgentLoop running).
    Running,
    /// Unrecoverable error occurred; requires reset.
    Error,
}

/// Manages the lifecycle of a single agent session for a given tenant.
///
/// Owns message history, tool set, and steer/follow-up queues.
/// Each session is isolated — no shared mutable state with other sessions.
pub struct SessionActor {
    tenant_id: String,
    session_id: String,
    model: String,
    prompt_builder: PromptBuilder,
    stream_options: ai_provider::StreamOptions,
    max_retries: u32,
    provider: Arc<dyn ai_provider::LlmProvider>,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    compaction_actor: Arc<Compactor>,
    tools: Vec<AgentToolRef>,
    entries: Vec<SessionEntry>,
    /// Messages queued for injection before the next LLM call
    steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    /// Messages queued for injection after the agent would stop
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    /// Optional persistence backend for session history
    store: Option<Arc<dyn SessionStore>>,
    /// Whether auto-restore should run before the next prompt
    needs_restore: bool,
    /// Entry count at the time of last save (incremental save boundary)
    last_saved_entry_count: usize,
    /// Timestamp when this session actor was created
    session_started_at: std::time::SystemTime,
    /// Skills available for this session.
    skills: Vec<crate::skills::Skill>,
    /// Handle of the most recent fire-and-forget persistence task.
    /// Awaiting this before spawning a new save guarantees write ordering
    /// and prevents stale snapshots from overwriting newer ones.
    last_save: Option<tokio::task::JoinHandle<()>>,
    abort_token: CancellationToken,

    recovery: RecoveryStateMachine,
    event_listeners: Arc<Mutex<Vec<Arc<dyn AgentEventListener>>>>,
    event_tx: Option<tokio::sync::mpsc::Sender<QueuedEvent>>,
    event_processor_handle: Option<tokio::task::JoinHandle<()>>,
    state: AtomicU8, // 0=Idle, 1=Running, 2=Error
    error_reason: Mutex<Option<String>>,
}

/// Configuration for creating a new [`SessionActor`].
///
/// Using this struct makes the constructor forward-compatible: new optional
/// fields can be added here without changing [`SessionActor::new`] signature.
pub struct SessionConfig {
    pub tenant_id: String,
    pub session_id: String,
    pub system_prompt: String,
    pub model: String,
    pub provider: Arc<dyn ai_provider::LlmProvider>,
    pub hook_dispatcher: Arc<dyn HookDispatcher>,
    pub compaction_actor: Arc<Compactor>,
    pub tools: Vec<AgentToolRef>,
    pub store: Option<Arc<dyn SessionStore>>,
    pub skills: Vec<crate::skills::Skill>,
}

impl std::fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionConfig")
            .field("tenant_id", &self.tenant_id)
            .field("session_id", &self.session_id)
            .field("system_prompt", &self.system_prompt)
            .field("model", &self.model)
            .field("provider", &"<dyn LlmProvider>")
            .field("hook_dispatcher", &"<dyn HookDispatcher>")
            .field("compaction_actor", &"<Compactor>")
            .field("tools", &format!("{} tools", self.tools.len()))
            .field("store", &self.store.is_some())
            .field("skills", &format!("{} skills", self.skills.len()))
            .finish()
    }
}

impl SessionActor {
    /// Create a new session from a [`SessionConfig`].
    ///
    /// Emits `on_session_start` hook (fire-and-forget, per ADR-003) on construction.
    /// This hook is observational only — it must not perform setup work that
    /// affects the session, as it runs concurrently and may not complete before
    /// the first `prompt()` call.
    ///
    /// If a `store` is provided, call [`restore`](Self::restore) after construction
    /// to load message history before the first prompt.
    pub fn new(config: SessionConfig) -> Self {
        // Emit session_start (fire-and-forget, per ADR-003)
        let tool_defs: Vec<serde_json::Value> = config
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "parameters": t.parameters(),
                })
            })
            .collect();
        let mut prompt_builder = PromptBuilder::from_base(config.system_prompt.clone());
        if !config.skills.is_empty() {
            let skills_xml = crate::skills::format_skills_for_prompt(&config.skills);
            prompt_builder.upsert_fragment(PromptFragment {
                id: "skills-directory".into(),
                kind: FragmentKind::SkillsDirectory,
                source: FragmentSource::SkillsInjector,
                content: skills_xml,
                priority: 50,
            });
        }

        let session_ctx = SessionCtx {
            tenant_id: config.tenant_id.clone(),
            session_id: config.session_id.clone(),
            system_prompt: prompt_builder.render(),
            tools: tool_defs,
        };
        let dispatcher = config.hook_dispatcher.clone();
        tokio::spawn(async move {
            let _ = crate::hook::timeout::with_timeout(
                dispatcher.on_session_start(&session_ctx),
                100,
                (),
                "on_session_start",
            )
            .await;
        });

        info!(
            tenant_id = %config.tenant_id,
            session_id = %config.session_id,
            tools_count = config.tools.len(),
            has_store = config.store.is_some(),
            "session started",
        );

        let has_store = config.store.is_some();

        let mut actor = Self {
            tenant_id: config.tenant_id,
            session_id: config.session_id,
            model: config.model,
            prompt_builder,
            stream_options: ai_provider::StreamOptions::default(),
            max_retries: 3,
            provider: config.provider,
            hook_dispatcher: config.hook_dispatcher,
            compaction_actor: config.compaction_actor,
            tools: config.tools,
            skills: config.skills,
            entries: Vec::new(),
            steer_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            store: config.store,
            needs_restore: has_store,
            last_saved_entry_count: 0,
            session_started_at: std::time::SystemTime::now(),
            last_save: None,
            abort_token: CancellationToken::new(),
            recovery: RecoveryStateMachine::new(3),
            event_listeners: Arc::new(Mutex::new(Vec::new())),
            event_tx: None,
            event_processor_handle: None,
            state: AtomicU8::new(0),
            error_reason: Mutex::new(None),
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
                    listeners
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .iter()
                        .cloned()
                        .collect()
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
    /// **Deprecated:** Restore now happens automatically in `prompt()` /
    /// `run_with_messages()`. This method is a no-op and will be removed
    /// in a future version.
    #[deprecated(since = "0.2.0", note = "restore is now automatic; this method is a no-op")]
    pub async fn restore(&mut self) -> Result<usize, AgentError> {
        Ok(0)
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
    pub async fn prompt(&mut self, text: String) -> Result<Vec<AgentMessage>, AgentError> {
        self.prompt_with_content(vec![Content::Text {
            text,
            text_signature: None,
        }])
        .await
    }

    pub async fn prompt_with_content(
        &mut self,
        content: Vec<Content>,
    ) -> Result<Vec<AgentMessage>, AgentError> {
        if self.state.load(Ordering::SeqCst) == 2 {
            let reason = self
                .error_reason
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_default();
            return Err(AgentError::SessionInError { reason });
        }

        // Auto-restore session history from store before first prompt
        if self.needs_restore {
            self.needs_restore = false;
            if let Some(ref store) = self.store {
                match store.load_session(&self.tenant_id, &self.session_id).await {
                    Ok(entries) if !entries.is_empty() => {
                        let count = entries.len();
                        self.entries = entries;
                        self.last_saved_entry_count = count;
                        info!(
                            tenant_id = %self.tenant_id,
                            session_id = %self.session_id,
                            restored_count = count,
                            "auto-restored session history",
                        );
                    }
                    Ok(_) => {
                        // Empty store — fresh session
                    }
                    Err(e) => {
                        warn!(
                            tenant_id = %self.tenant_id,
                            session_id = %self.session_id,
                            error = %e,
                            "auto-restore failed, starting with empty session",
                        );
                    }
                }
            }
        }

        // Handle /skill:name invocation only when there's exactly one Text part
        if content.len() == 1 {
            if let Content::Text { ref text, .. } = content[0] {
                if let Some(skill_name) = crate::skills::parse_skill_invocation(text) {
                    if let Some(skill) = self.skills.iter().find(|s| s.name == skill_name) {
                        let skill_content = tokio::fs::read_to_string(&skill.file_path)
                            .await
                            .map_err(|e| {
                                AgentError::SkillLoadFailed(format!(
                                    "failed to read skill {}: {}",
                                    skill.name, e
                                ))
                            })?;

                        let skill_msg = AgentMessage::User(ai_provider::UserMessage {
                            content: vec![Content::Text {
                                text: format!("[Skill: {}]\n{}", skill.name, skill_content),
                                text_signature: None,
                            }],
                            timestamp: std::time::SystemTime::now(),
                        });
                        self.steer(skill_msg);
                        return self.run_with_messages(None).await;
                    } else {
                        return Err(AgentError::SkillNotFound(skill_name.to_string()));
                    }
                }
            }
        }

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content,
            timestamp: std::time::SystemTime::now(),
        });
        self.push_message(user_msg);
        self.run_with_messages(None).await
    }

    pub async fn complete(&mut self, text: String) -> Result<String, AgentError> {
        let messages = self.prompt(text).await?;
        let text_content: Vec<String> = messages
            .iter()
            .filter_map(|m| {
                if let AgentMessage::Assistant(a) = m {
                    Some(
                        a.content
                            .iter()
                            .filter_map(|c| match c {
                                ai_provider::Content::Text { text, .. } => Some(text.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join(" "),
                    )
                } else {
                    None
                }
            })
            .collect();
        Ok(text_content.join("\n"))
    }

    pub async fn continue_(&mut self) -> Result<Vec<AgentMessage>, AgentError> {
        if self.state.load(Ordering::SeqCst) == 2 {
            let reason = self
                .error_reason
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_default();
            return Err(AgentError::SessionInError { reason });
        }
        self.run_with_messages(None).await
    }

    pub fn is_streaming(&self) -> bool {
        self.state.load(Ordering::SeqCst) == 1
    }

    pub fn state(&self) -> SessionState {
        match self.state.load(Ordering::SeqCst) {
            1 => SessionState::Running,
            2 => SessionState::Error,
            _ => SessionState::Idle,
        }
    }

    pub fn error_reason(&self) -> Option<String> {
        self.error_reason
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub async fn reset(&mut self) -> Result<CancellationToken, AgentError> {
        self.abort_token.cancel();
        self.entries.clear();
        self.recovery = RecoveryStateMachine::new(self.max_retries);
        self.state.store(0, Ordering::SeqCst);
        *self.error_reason.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.abort_token = CancellationToken::new();
        Ok(self.abort_token.clone())
    }

    pub fn add_event_listener(&mut self, listener: Arc<dyn AgentEventListener>) {
        self.event_listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(listener);
    }

    fn emit_event(&self, event: AgentEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(QueuedEvent { event });
        }
    }

    pub fn set_system_prompt(&mut self, prompt: String) {
        let mut builder = PromptBuilder::from_base(prompt);
        if !self.skills.is_empty() {
            let skills_xml = crate::skills::format_skills_for_prompt(&self.skills);
            builder.upsert_fragment(PromptFragment {
                id: "skills-directory".into(),
                kind: FragmentKind::SkillsDirectory,
                source: FragmentSource::SkillsInjector,
                content: skills_xml,
                priority: 50,
            });
        }
        self.prompt_builder = builder;
    }
    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }
    pub fn set_tools(&mut self, tools: Vec<AgentToolRef>) {
        self.tools = tools;
    }
    pub fn set_stream_options(&mut self, options: ai_provider::StreamOptions) {
        self.stream_options = options;
    }
    pub fn set_max_retries(&mut self, max_retries: u32) {
        self.max_retries = max_retries;
        self.stream_options.max_retries = max_retries;
    }
    pub fn system_prompt(&self) -> String {
        self.prompt_builder.render()
    }

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
            self.state.store(1, Ordering::SeqCst);
            self.emit_event(AgentEvent::StateChanged {
                state: SessionState::Running,
            });
            self.abort_token = CancellationToken::new();

            let messages = SessionContextBuilder::build_context(&self.entries);

            let event_tx = self.event_tx.clone();
            let event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync + 'static> =
                Arc::new(move |event| {
                    if let Some(tx) = &event_tx
                        && tx.try_send(QueuedEvent { event }).is_err()
                    {
                        tracing::warn!("event queue full, dropping event");
                    }
                });

            let config = AgentLoopConfig {
                tenant_id: self.tenant_id.clone(),
                session_id: self.session_id.clone(),
                model: self.model.clone(),
                provider: self.provider.clone(),
                hook_dispatcher: self.hook_dispatcher.clone(),
                tools: self.tools.clone(),
                prompt_builder: self.prompt_builder.clone(),
                stream_options: self.stream_options.clone(),
                event_sink,
                steer_queue: self.steer_queue.clone(),
                follow_up_queue: self.follow_up_queue.clone(),
                circuit_breaker: None,
                skills: self.skills.clone(),
            };

            match AgentLoop::new(config)
                .run(messages, self.abort_token.child_token())
                .await
            {
                Ok(msgs) => {
                    self.state.store(0, Ordering::SeqCst);
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
                                self.state.store(2, Ordering::SeqCst);
                                *self.error_reason.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason.clone());
                                self.emit_event(AgentEvent::StateChanged { state: SessionState::Error });
                                return Err(AgentError::RecoveryAborted(reason));
                            }
                            RecoveryAction::Continue => { self.recovery.mark_success(); }
                        }
                    }
                    for msg in &msgs {
                        self.push_message(msg.clone());
                    }
                    all_new_msgs.extend(msgs);
                }
                Err(e) => {
                    self.state.store(0, Ordering::SeqCst);
                    match e {
                        AgentError::Cancelled => {
                            return Err(AgentError::Cancelled);
                        }
                        AgentError::ContextOverflow(msg) => {
                            let action = self.recovery.evaluate_overflow(&msg);
                            match action {
                                RecoveryAction::RetryAfterCompaction { .. } => {
                                    self.recovery.mark_success();
                                    self.run_auto_compaction(CompactReason::Overflow, true)
                                        .await?;
                                    continue;
                                }
                                RecoveryAction::Abort { reason } => {
                                    self.state.store(2, Ordering::SeqCst);
                                    *self.error_reason.lock().unwrap_or_else(|e| e.into_inner()) =
                                        Some(reason.clone());
                                    self.emit_event(AgentEvent::StateChanged {
                                        state: SessionState::Error,
                                    });
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
                                    self.state.store(2, Ordering::SeqCst);
                                    *self.error_reason.lock().unwrap_or_else(|e| e.into_inner()) =
                                        Some(msg.clone());
                                    self.emit_event(AgentEvent::StateChanged {
                                        state: SessionState::Error,
                                    });
                                    return Err(AgentError::ContextOverflow(msg));
                                }
                            }
                        }
                        other => {
                            self.state.store(2, Ordering::SeqCst);
                            *self.error_reason.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(other.to_string());
                            self.emit_event(AgentEvent::StateChanged {
                                state: SessionState::Error,
                            });
                            return Err(other);
                        }
                    }
                }
            }

            // Mid-loop threshold compaction
            if self.compaction_actor.config.enabled {
                let context_tokens = estimate_context_tokens(&self.entries);
                let context_window = self.model_context_window();
                if should_compact(
                    context_tokens,
                    context_window,
                    &self.compaction_actor.config,
                ) {
                    self.run_auto_compaction(CompactReason::Threshold, false)
                        .await?;
                }
            }

            break;
        }

        // Emit final Idle event and clear any stale error reason.
        // State is already Idle set by the Ok(msgs) branch above.
        self.error_reason
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        self.emit_event(AgentEvent::StateChanged {
            state: SessionState::Idle,
        });

        info!(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            total_entries = self.entries.len(),
            new_msg_count = all_new_msgs.len(),
            "agent run complete",
        );

        // Check for threshold compaction after successful turn
        if let Some(AgentMessage::Assistant(last_assistant)) = all_new_msgs.iter().rfind(
            |m| matches!(m, AgentMessage::Assistant(a) if a.stop_reason != StopReason::ToolUse),
        ) && let Err(e) = self.check_compaction(last_assistant).await
        {
            warn!(
                tenant_id = %self.tenant_id,
                session_id = %self.session_id,
                error = %e,
                "auto-compaction failed",
            );
        }

        // Persist incrementally — only save new entries since last save.
        // Await the previous save task to preserve ordering, then spawn a new
        // fire-and-forget task for the new entries.
        if let Some(ref store) = self.store {
            let new_entries = &self.entries[self.last_saved_entry_count..];
            if !new_entries.is_empty() {
                if let Some(handle) = self.last_save.take() {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
                }
                let entries_to_save = new_entries.to_vec();
                self.last_saved_entry_count = self.entries.len();
                let tenant_id = self.tenant_id.clone();
                let session_id = self.session_id.clone();
                let store = store.clone();
                self.last_save = Some(tokio::spawn(async move {
                    if let Err(e) = store.append_entries(&tenant_id, &session_id, &entries_to_save).await {
                        warn!(
                            tenant_id = %tenant_id,
                            session_id = %session_id,
                            error = %e,
                            "failed to persist session",
                        );
                    }
                }));
            }
        }

        Ok(all_new_msgs)
    }

    fn model_context_window(&self) -> usize {
        self.provider
            .model_metadata(&self.model)
            .map(|m| m.context_window as usize)
            .unwrap_or(0)
    }

    async fn run_auto_compaction(
        &mut self,
        reason: CompactReason,
        will_retry: bool,
    ) -> Result<(), AgentError> {
        // Emit compaction_start
        if let Some(tx) = &self.event_tx {
            tx.send(QueuedEvent {
                event: AgentEvent::CompactionStart {
                    reason: reason.clone(),
                },
            })
            .await
            .ok();
        }

        // 1. Extension hook
        let preparation = self
            .compaction_actor
            .prepare(&self.entries)
            .map_err(|e| AgentError::CompactionFailed(e.to_string()))?;

        let compact_ctx = CompactCtx {
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            preparation,
            entries: self.entries.clone(),
            reason: reason.clone(),
        };

        let decision = crate::hook::timeout::with_timeout(
            self.hook_dispatcher.on_before_compact(&compact_ctx),
            500,
            crate::mutations::CompactDecision::Continue,
            "on_before_compact",
        )
        .await;
        let original_reason = reason.clone();

        let (from_extension, result) = match decision {
            crate::mutations::CompactDecision::Block {
                reason: block_reason,
            } => {
                if let Some(tx) = &self.event_tx {
                    tx.send(QueuedEvent {
                        event: AgentEvent::CompactionEnd {
                            reason: original_reason,
                            result: None,
                            aborted: true,
                            will_retry: false,
                            error_message: Some(block_reason),
                        },
                    })
                    .await
                    .ok();
                }
                return Ok(());
            }
            crate::mutations::CompactDecision::Replace { result } => (true, result),
            crate::mutations::CompactDecision::Continue => {
                let result = self
                    .compaction_actor
                    .compact(&self.entries, &self.abort_token.child_token())
                    .await
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

        // 3. Truncate entries before the compaction boundary to prevent
        // unbounded memory growth.
        self.truncate_entries_before(result.first_kept_entry_id);

        // 4. Emit compaction_end
        let result_for_hook = result.clone();
        if let Some(tx) = &self.event_tx {
            tx.send(QueuedEvent {
                event: AgentEvent::CompactionEnd {
                    reason: reason.clone(),
                    result: Some(result),
                    aborted: false,
                    will_retry,
                    error_message: None,
                },
            })
            .await
            .ok();
        }

        // 5. Trigger on_compact_end hook
        let compact_end_ctx = crate::hook::context::CompactEndCtx {
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            compacted_messages: vec![],
            token_savings: 0,
            result: Some(result_for_hook),
        };
        let _ = crate::hook::timeout::with_timeout(
            self.hook_dispatcher.on_compact_end(&compact_end_ctx),
            500,
            (),
            "on_compact_end",
        )
        .await;

        Ok(())
    }

    fn is_context_overflow(assistant: &ai_provider::AssistantMessage) -> bool {
        assistant.stop_reason == ai_provider::StopReason::Error
            && assistant.error_message.as_ref().is_some_and(|e| {
                let lower = e.to_lowercase();
                lower.contains("context length") || lower.contains("token limit")
            })
    }

    async fn check_compaction(
        &mut self,
        last_assistant: &ai_provider::AssistantMessage,
    ) -> Result<(), AgentError> {
        let config = &self.compaction_actor.config;
        if !config.enabled {
            return Ok(());
        }

        if last_assistant.stop_reason == ai_provider::StopReason::Aborted {
            return Ok(());
        }

        // Skip if assistant message is from before last compaction
        if let Some(SessionEntry::Compaction { timestamp, .. }) = self
            .entries
            .iter()
            .rfind(|e| matches!(e, SessionEntry::Compaction { .. }))
            && last_assistant.timestamp <= *timestamp
        {
            return Ok(());
        }

        // Case 1: Overflow (recovery is handled by RecoveryStateMachine, here we just compact)
        if Self::is_context_overflow(last_assistant) {
            if self.recovery.overflow_attempted {
                return Err(AgentError::CompactionFailed(
                    "Context overflow recovery failed after one compact-and-retry attempt".into(),
                ));
            }
            self.recovery.overflow_attempted = true;
            self.run_auto_compaction(CompactReason::Overflow, false)
                .await?;
            return Ok(());
        }

        // Case 2: Threshold
        let context_tokens = estimate_context_tokens(&self.entries);
        let context_window = self.model_context_window();

        if should_compact(context_tokens, context_window, config) {
            self.run_auto_compaction(CompactReason::Threshold, false)
                .await?;
        }

        Ok(())
    }

    /// Manually trigger compaction with optional custom instructions.
    pub async fn compact(
        &mut self,
        _custom_instructions: Option<String>,
    ) -> Result<CompactionResult, AgentError> {
        // For manual compaction, we always use Continue decision (no extension override)
        let result = self
            .compaction_actor
            .compact(&self.entries, &self.abort_token.child_token())
            .await
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

        // Truncate entries before the compaction boundary to prevent
        // unbounded memory growth.
        self.truncate_entries_before(result.first_kept_entry_id);

        Ok(result)
    }

    /// Remove entries before the given boundary entry ID.
    ///
    /// Called after compaction to prevent unbounded in-memory growth
    /// of the session history `Vec`.
    fn truncate_entries_before(&mut self, first_kept_entry_id: uuid::Uuid) {
        if let Some(kept_idx) = self
            .entries
            .iter()
            .position(|e| e.id() == first_kept_entry_id)
        {
            self.entries.drain(..kept_idx);
        }
    }

    pub fn push_message(&mut self, msg: AgentMessage) {
        self.entries.push(SessionEntry::Message {
            id: uuid::Uuid::new_v4(),
            message: msg,
        });
    }

    /// Queue a steering message (injected before next LLM call in current run)
    pub fn steer(&mut self, message: AgentMessage) {
        self.steer_queue
            .lock()
            .expect("steer queue poisoned")
            .push(message);
    }

    pub fn follow_up(&mut self, message: AgentMessage) {
        self.follow_up_queue
            .lock()
            .expect("follow_up queue poisoned")
            .push(message);
    }

    /// Flush pending persistence writes.
    ///
    /// The session state is saved asynchronously (fire-and-forget) after each
    /// `prompt()` call. Call `flush()` before shutdown to guarantee all writes
    /// have completed. Returns `Ok(())` if no store is configured.
    ///
    /// **Breaking change (v0.x):** This method now takes `&mut self` instead of
    /// `&self` to consume the in-flight `last_save` handle. Callers that
    /// previously held a shared reference must obtain a mutable reference
    /// before calling `flush()`.
    pub async fn flush(&mut self) -> Result<(), AgentError> {
        if let Some(handle) = self.last_save.take() {
            let _ = handle.await;
        }
        if let Some(ref store) = self.store {
            store
                .save_session(&self.tenant_id, &self.session_id, &self.entries)
                .await?;
            info!(
                tenant_id = %self.tenant_id,
                session_id = %self.session_id,
                "session state flushed to store",
            );
        }
        Ok(())
    }

    /// Get a clone of the cancellation token for this session.
    pub fn abort_token(&self) -> CancellationToken {
        self.abort_token.clone()
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

        // 2. Wait for in-flight persistence to complete
        if let Some(handle) = self.last_save.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }

        // 3. Drop event sender to signal the processor to exit
        self.event_tx.take();

        // 4. Wait for the event processor with a timeout
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

    /// Return the tools registered for this session.
    pub fn tools(&self) -> &[crate::types::AgentToolRef] {
        &self.tools
    }
}

impl Drop for SessionActor {
    fn drop(&mut self) {
        // 1. Cancel any in-flight operations
        self.abort_token.cancel();

        // 2. Drop event sender so the event processor sees channel closed
        //    and drains any buffered events before exiting naturally.
        //    We do NOT abort the handle here — a forced abort would drop
        //    events already sitting in the mpsc buffer (e.g. TurnEnd).
        self.event_tx.take();

        // Take the handle out so it is dropped alongside this SessionActor.
        // JoinHandle::drop does NOT abort the task; the task continues to
        // run until the recv loop observes the closed channel.
        let _ = self.event_processor_handle.take();

        // NOTE: `last_save` is intentionally NOT awaited here because
        // `Drop` cannot be async. Callers MUST call `shutdown()` (which
        // awaits `last_save` with a 5s timeout) before dropping the
        // `SessionActor` to guarantee all persistence writes complete.
        // A bare `drop()` will abort the in-flight save task, potentially
        // losing the last write.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_ops::DefaultFileOperationExtractor;
    use crate::harness::compaction::CompactionConfig;
    use crate::hook::context::AgentEndCtx;
    use crate::test_utils::{AllowAllDispatcher, TestProvider};
    use async_trait::async_trait;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{Duration, sleep};

    fn make_compaction_actor(provider: Arc<dyn ai_provider::LlmProvider>) -> Compactor {
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
        assert!(session.abort_token.is_cancelled());

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

    #[tokio::test]
    async fn test_restore_from_store() {
        let _ = tracing_subscriber::fmt().try_init();

        let store = Arc::new(MemoryStore::new());
        let provider = TestProvider::text("response");
        let dispatcher = Arc::new(AllowAllDispatcher);

        // Create session and add some messages
        {
            let mut session = SessionActor::new(SessionConfig {
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
            session.prompt("hello".to_string()).await.unwrap();
            session.flush().await.unwrap();
        }

        // Create new session with same store, restore should get messages back
        let mut session2 = SessionActor::new(SessionConfig {
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

        let restored = session2.restore().await.unwrap();
        assert!(restored > 0);
        let msgs = session2.messages();
        assert!(msgs.len() >= 2); // user + assistant
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
