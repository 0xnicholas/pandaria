//! Session module — owns the per-tenant session lifecycle.
//!
//! Structure:
//! - [`SessionActor`]: slim orchestrator (17 fields)
//! - [`SessionHistory`]: message history + persistence (~280 lines)
//! - [`SessionEventHub`]: events + steer/follow-up queues (~220 lines)
//! - [`SessionStateMachine`]: state + error + recovery + cancel (~180 lines)

pub mod event_hub;
pub mod history;
pub mod state;

pub use event_hub::SessionEventHub;
pub use history::SessionHistory;
pub use state::SessionStateMachine;

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
use crate::harness::error_recovery::RecoveryAction;
use crate::harness::strategy::{
    ContextStrategy, CriteriaEvaluation, DEFAULT_LOOP_INTERVAL, GoalCriterion, GoalExhaustedAction,
    GoalOutcome, GoalVerification, RhythmStrategy, SessionStrategy, TerminationStrategy,
};
use crate::hook::context::{CompactCtx, CompactReason, SessionCtx};
use crate::hook::dispatcher::HookDispatcher;
use crate::hook::with_timeout_from;
use crate::persistence::entry::{SessionContextBuilder, SessionEntry};
use crate::persistence::store::SessionStore;
use crate::prompt::{FragmentKind, FragmentSource, PromptBuilder, PromptFragment};
use crate::types::{AgentMessage, AgentToolRef};

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
    // ── Identity & Config (top-level, accessed by most methods) ──
    tenant_id: String,
    session_id: String,
    model: String,
    prompt_builder: PromptBuilder,
    stream_options: ai_provider::StreamOptions,
    max_retries: u32,
    /// Saved base persona for context-clear rebuilds.
    base_persona: String,
    /// Skills available for this session.
    skills: Vec<crate::skills::Skill>,

    // ── LLM Wiring ──
    provider: Arc<dyn ai_provider::LlmProvider>,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    compaction_actor: Arc<Compactor>,
    tools: Vec<AgentToolRef>,

    // ── Strategy & Bookkeeping ──
    /// Execution strategy for this session.
    strategy: SessionStrategy,
    /// Token usage from the most recent agent loop turn.
    last_usage: Option<ai_provider::Usage>,

    // ── Subsystems (Task 6: legacy fields removed, subsystems own the data) ──
    /// Message history + persistence + restore + flush.
    history: history::SessionHistory,
    /// Events + steer/follow-up queues + processor.
    event_hub: event_hub::SessionEventHub,
    /// State machine + error reason + recovery + abort token.
    state_machine: state::SessionStateMachine,
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

        let base_persona = config.system_prompt.clone();
        let history = history::SessionHistory::new(
            config.tenant_id.clone(),
            config.session_id.clone(),
            config.store.clone(),
        );

        Self {
            tenant_id: config.tenant_id,
            session_id: config.session_id,
            model: config.model,
            prompt_builder,
            stream_options: ai_provider::StreamOptions::default(),
            max_retries: 3,
            base_persona,
            skills: config.skills,
            provider: config.provider,
            hook_dispatcher: config.hook_dispatcher,
            compaction_actor: config.compaction_actor,
            tools: config.tools,
            strategy: SessionStrategy::default(),
            last_usage: None,
            history,
            event_hub: event_hub::SessionEventHub::new(),
            state_machine: state::SessionStateMachine::new(3),
        }
    }

    /// Attempt to restore session history from the configured store.
    ///
    /// **Deprecated:** Restore now happens automatically in `prompt()` /
    /// `run_with_messages()`. This method is a no-op and will be removed
    /// in a future version.
    #[deprecated(
        since = "0.2.0",
        note = "restore is now automatic; this method is a no-op"
    )]
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
        let content = vec![Content::Text {
            text: text.clone(),
            text_signature: None,
        }];

        // Extract strategy fields before matching to avoid borrow conflicts
        let termination = self.strategy.termination.clone();
        let rhythm = self.strategy.rhythm.clone();

        match (&termination, &rhythm) {
            // ── Default: single-shot ──
            (TerminationStrategy::Once, RhythmStrategy::Once) => {
                self.prompt_with_content(content).await
            }

            // ── Goal: sync verification loop ──
            (TerminationStrategy::Goal { .. }, RhythmStrategy::Once) => {
                let outcome = self.run_goal_sync(text).await?;
                Ok(outcome.into_messages())
            }

            // ── Loop: background execution ──
            (
                _,
                RhythmStrategy::Loop {
                    interval,
                    max_iterations,
                },
            ) => {
                if std::env::var("PANDARIA_DISABLE_CRON").as_deref() == Ok("1") {
                    return Err(AgentError::LoopDisabled);
                }
                let delay = interval.unwrap_or(DEFAULT_LOOP_INTERVAL);
                let max = *max_iterations;

                // First iteration: synchronous
                let first = match &termination {
                    TerminationStrategy::Once => self.prompt_with_content(content).await?,
                    TerminationStrategy::Goal { .. } => {
                        self.run_goal_sync(text.clone()).await?.into_messages()
                    }
                    #[allow(unreachable_patterns)]
                    _ => return Err(AgentError::LoopDisabled),
                };

                // Subsequent iterations: background
                self.spawn_background_loop(text, delay, max);

                Ok(first)
            }
        }
    }

    pub async fn prompt_with_content(
        &mut self,
        content: Vec<Content>,
    ) -> Result<Vec<AgentMessage>, AgentError> {
        if self.state_machine.state() == SessionState::Error {
            let reason = self.state_machine.error_reason().unwrap_or_default();
            return Err(AgentError::SessionInError { reason });
        }

        // Auto-restore session history from store before first prompt (delegated to SessionHistory)
        self.history.auto_restore().await?;

        // Handle /skill:name invocation only when there's exactly one Text part
        if content.len() == 1
            && let Content::Text { ref text, .. } = content[0]
            && let Some(skill_name) = crate::skills::parse_skill_invocation(text)
        {
            if let Some(skill) = self.skills.iter().find(|s| s.name == skill_name) {
                let skill_content =
                    tokio::fs::read_to_string(&skill.file_path)
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

    /// Like [`complete`](Self::complete) but also captures per-chunk text deltas
    /// from the LLM streaming response.
    ///
    /// Returns both the accumulated full text and a vector of intermediate
    /// text deltas received during streaming. The deltas represent incremental
    /// LLM output chunks (e.g. one per token or word).
    pub async fn complete_with_deltas(
        &mut self,
        text: String,
    ) -> Result<(String, Vec<String>), AgentError> {
        use std::sync::Mutex as StdMutex;

        // Add user message (same as prompt)
        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: text.clone(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });
        self.push_message(user_msg);

        let deltas: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
        let _deltas_capture = deltas.clone();

        // Run with a text_stream_tx that captures deltas
        let messages = {
            self.state_machine.enter_running();
            self.persist_status("active");
            self.emit_event(AgentEvent::StateChanged {
                state: SessionState::Running,
            });
            self.state_machine.reset_abort_token();

            let ctx = SessionContextBuilder::build_context(self.history.entries());
            let event_tx = self.event_hub.event_tx_clone();
            let event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync + 'static> =
                Arc::new(move |event| {
                    if let Some(tx) = &event_tx
                        && tx.try_send(event_hub::QueuedEvent { event }).is_err()
                    {
                        tracing::warn!("event queue full, dropping event");
                    }
                });

            let (text_tx, mut text_rx) = tokio::sync::mpsc::unbounded_channel();

            // Spawn a task to collect deltas from the channel
            let deltas_for_task = deltas.clone();
            tokio::spawn(async move {
                while let Some(delta) = text_rx.recv().await {
                    deltas_for_task.lock().unwrap().push(delta);
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
                steer_queue: self.event_hub.steer_queue_clone(),
                follow_up_queue: self.event_hub.follow_up_queue_clone(),
                circuit_breaker: None,
                skills: self.skills.clone(),
                text_stream_tx: Some(text_tx),
            };

            let result = AgentLoop::new(config)
                .run(ctx, self.state_machine.child_token())
                .await;

            // Drop the sender to close the channel and let the collector task finish
            // (text_tx is already consumed by AgentLoop, the Drop happens when AgentLoop is done)

            match result {
                Ok(msgs) => {
                    self.state_machine.enter_idle();
                    // Capture last turn's usage
                    if let Some(AgentMessage::Assistant(a)) = msgs.last() {
                        self.last_usage = Some(a.usage.clone());
                    }
                    for msg in &msgs {
                        self.push_message(msg.clone());
                    }
                    msgs
                }
                Err(e) => {
                    self.state_machine.enter_idle();
                    return Err(e);
                }
            }
        };

        // Collect text content
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

        let deltas = deltas.lock().unwrap().clone();

        Ok((text_content.join("\n"), deltas))
    }

    pub async fn continue_(&mut self) -> Result<Vec<AgentMessage>, AgentError> {
        if self.state_machine.state() == SessionState::Error {
            let reason = self.state_machine.error_reason().unwrap_or_default();
            return Err(AgentError::SessionInError { reason });
        }
        self.run_with_messages(None).await
    }

    pub fn is_streaming(&self) -> bool {
        self.state_machine.is_streaming()
    }

    pub fn state(&self) -> SessionState {
        self.state_machine.state()
    }

    pub fn error_reason(&self) -> Option<String> {
        self.state_machine.error_reason()
    }

    pub async fn reset(&mut self) -> Result<CancellationToken, AgentError> {
        self.state_machine.abort();
        self.history.clear_entries();
        self.state_machine.reset_recovery_only(self.max_retries);
        self.state_machine.enter_idle();
        self.state_machine.clear_error();
        Ok(self.state_machine.abort_token())
    }

    pub fn add_event_listener(&mut self, listener: Arc<dyn AgentEventListener>) {
        self.event_hub.add_listener(listener);
    }

    fn emit_event(&self, event: AgentEvent) {
        self.event_hub.emit(event);
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
            self.state_machine.enter_running();
            self.persist_status("active");
            self.emit_event(AgentEvent::StateChanged {
                state: SessionState::Running,
            });
            self.state_machine.reset_abort_token();

            let messages = SessionContextBuilder::build_context(self.history.entries());

            let event_tx = self.event_hub.event_tx_clone();
            let event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync + 'static> =
                Arc::new(move |event| {
                    if let Some(tx) = &event_tx
                        && tx.try_send(event_hub::QueuedEvent { event }).is_err()
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
                steer_queue: self.event_hub.steer_queue_clone(),
                follow_up_queue: self.event_hub.follow_up_queue_clone(),
                circuit_breaker: None,
                skills: self.skills.clone(),
                text_stream_tx: None,
            };

            match AgentLoop::new(config)
                .run(messages, self.state_machine.child_token())
                .await
            {
                Ok(msgs) => {
                    self.state_machine.enter_idle();
                    // Capture last turn's usage for external consumers
                    if let Some(AgentMessage::Assistant(a)) = msgs.last() {
                        self.last_usage = Some(a.usage.clone());
                    }
                    if let Some(AgentMessage::Assistant(assistant)) = msgs.iter().rfind(|m| matches!(m, AgentMessage::Assistant(a) if a.stop_reason != StopReason::ToolUse)) {
                        let action = self.state_machine.recovery_mut().evaluate(assistant);
                        match action {
                            RecoveryAction::RetryAfterBackoff { delay_ms } => {
                                let cancel_token = self.state_machine.abort_token();
                                tokio::select! {
                                    _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                                    _ = cancel_token.cancelled() => {
                                        self.state_machine.recovery_mut().mark_success();
                                        return Err(AgentError::RecoveryAborted(
                                            "recovery aborted via backoff timeout".into()
                                        ));
                                    }
                                }
                                self.state_machine.recovery_mut().mark_success();
                                continue;
                            }
                            RecoveryAction::RetryAfterCompaction { .. } => {
                                self.state_machine.recovery_mut().mark_success();
                                self.run_auto_compaction(CompactReason::Overflow, true).await?;
                                continue;
                            }
                            RecoveryAction::Abort { reason } => {
                                self.state_machine.recovery_mut().mark_success();
                                                                self.state_machine.enter_error(reason.clone());
                                self.emit_event(AgentEvent::StateChanged { state: SessionState::Error });
                                return Err(AgentError::RecoveryAborted(reason));
                            }
                            RecoveryAction::Continue => { self.state_machine.recovery_mut().mark_success(); }
                        }
                    }
                    for msg in &msgs {
                        self.push_message(msg.clone());
                    }
                    all_new_msgs.extend(msgs);
                }
                Err(e) => {
                    self.state_machine.enter_idle();
                    match e {
                        AgentError::Cancelled => {
                            self.persist_status("aborted");
                            return Err(AgentError::Cancelled);
                        }
                        AgentError::ContextOverflow(msg) => {
                            let action = self.state_machine.recovery_mut().evaluate_overflow(&msg);
                            match action {
                                RecoveryAction::RetryAfterCompaction { .. } => {
                                    self.state_machine.recovery_mut().mark_success();
                                    self.run_auto_compaction(CompactReason::Overflow, true)
                                        .await?;
                                    continue;
                                }
                                RecoveryAction::Abort { reason } => {
                                    self.state_machine.enter_error(reason.clone());
                                    self.emit_event(AgentEvent::StateChanged {
                                        state: SessionState::Error,
                                    });
                                    return Err(AgentError::CompactionFailed(reason));
                                }
                                RecoveryAction::RetryAfterBackoff { delay_ms } => {
                                    let cancel_token = self.state_machine.abort_token();
                                    tokio::select! {
                                        _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                                        _ = cancel_token.cancelled() => {
                                            self.state_machine.recovery_mut().mark_success();
                                            return Err(AgentError::RecoveryAborted(
                                                "recovery aborted via backoff timeout".into()
                                            ));
                                        }
                                    }
                                    self.state_machine.recovery_mut().mark_success();
                                    continue;
                                }
                                RecoveryAction::Continue => {
                                    self.state_machine.enter_error(msg.clone());
                                    self.emit_event(AgentEvent::StateChanged {
                                        state: SessionState::Error,
                                    });
                                    return Err(AgentError::ContextOverflow(msg));
                                }
                            }
                        }
                        other => {
                            self.state_machine.enter_error(other.to_string());
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
                let context_tokens = estimate_context_tokens(self.history.entries());
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

        self.persist_status("completed");

        // Emit final Idle event and clear any stale error reason.
        // State is already Idle set by the Ok(msgs) branch above.
        self.state_machine.clear_error();
        self.emit_event(AgentEvent::StateChanged {
            state: SessionState::Idle,
        });

        info!(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            total_entries = self.history.entries().len(),
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
        self.history.persist_incremental().await;

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
        // Emit compaction_start via event_hub
        self.event_hub.emit(AgentEvent::CompactionStart {
            reason: reason.clone(),
        });

        // 1. Extension hook
        let preparation = self
            .compaction_actor
            .prepare(self.history.entries())
            .map_err(|e| AgentError::CompactionFailed(e.to_string()))?;

        let compact_ctx = CompactCtx {
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            preparation,
            entries: self.history.entries_clone(),
            reason: reason.clone(),
        };

        let decision = with_timeout_from(
            &*self.hook_dispatcher,
            self.hook_dispatcher.on_before_compact(&compact_ctx),
            crate::mutations::CompactDecision::Continue,
            "on_before_compact",
        )
        .await;
        let original_reason = reason.clone();

        let (from_extension, result) = match decision {
            crate::mutations::CompactDecision::Block {
                reason: block_reason,
            } => {
                self.event_hub.emit(AgentEvent::CompactionEnd {
                    reason: original_reason,
                    result: None,
                    aborted: true,
                    will_retry: false,
                    error_message: Some(block_reason),
                });
                return Ok(());
            }
            crate::mutations::CompactDecision::Replace { result } => (true, result),
            crate::mutations::CompactDecision::Continue => {
                let result = self
                    .compaction_actor
                    .compact(self.history.entries(), &self.state_machine.child_token())
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
        self.history.append_compaction_entry(compaction_entry);

        // 3. Truncate entries before the compaction boundary to prevent
        // unbounded memory growth.
        self.truncate_entries_before(result.first_kept_entry_id);

        // 4. Emit compaction_end via event_hub
        let result_for_hook = result.clone();
        self.event_hub.emit(AgentEvent::CompactionEnd {
            reason: reason.clone(),
            result: Some(result),
            aborted: false,
            will_retry,
            error_message: None,
        });

        // 5. Trigger on_compact_end hook
        let compact_end_ctx = crate::hook::context::CompactEndCtx {
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            compacted_messages: vec![],
            token_savings: 0,
            result: Some(result_for_hook),
        };
        let _ = with_timeout_from(
            &*self.hook_dispatcher,
            self.hook_dispatcher.on_compact_end(&compact_end_ctx),
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
        if let Some(ts) = self.history.last_compaction_timestamp()
            && last_assistant.timestamp <= ts
        {
            return Ok(());
        }

        // Case 1: Overflow (recovery is handled by RecoveryStateMachine, here we just compact)
        if Self::is_context_overflow(last_assistant) {
            if self.state_machine.recovery_mut().overflow_attempted {
                return Err(AgentError::CompactionFailed(
                    "Context overflow recovery failed after one compact-and-retry attempt".into(),
                ));
            }
            self.state_machine.recovery_mut().overflow_attempted = true;
            self.run_auto_compaction(CompactReason::Overflow, false)
                .await?;
            return Ok(());
        }

        // Case 2: Threshold
        let context_tokens = estimate_context_tokens(self.history.entries());
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
            .compact(self.history.entries(), &self.state_machine.child_token())
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
        self.history.append_compaction_entry(compaction_entry);

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
        self.history.truncate_before(first_kept_entry_id);
    }

    pub fn push_message(&mut self, msg: AgentMessage) {
        self.history.push(msg);
    }

    /// Queue a steering message (injected before next LLM call in current run)
    pub fn steer(&mut self, message: AgentMessage) {
        self.event_hub.steer(message);
    }

    pub fn follow_up(&mut self, message: AgentMessage) {
        self.event_hub.follow_up(message);
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
        if let Some(handle) = self.history.take_last_save() {
            let _ = handle.await;
        }
        self.history.flush().await?;
        info!(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            "session state flushed to store",
        );
        Ok(())
    }

    /// Get a clone of the cancellation token for this session.
    pub fn abort_token(&self) -> CancellationToken {
        self.state_machine.abort_token()
    }

    /// Abort the current run
    pub fn abort(&self) {
        warn!(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            "session aborted",
        );
        self.persist_status("aborted");
        self.state_machine.abort();
    }

    /// Set the execution strategy for this session.
    pub fn set_strategy(&mut self, strategy: SessionStrategy) {
        self.strategy = strategy;
    }

    /// Get the current execution strategy.
    pub fn strategy(&self) -> &SessionStrategy {
        &self.strategy
    }

    // ═══════════════════════════════════════════════════════════════
    // Strategy: Goal — synchronous verification loop
    // ═══════════════════════════════════════════════════════════════

    async fn run_goal_sync(&mut self, task: String) -> Result<GoalOutcome, AgentError> {
        let (criteria, max_attempts, on_exhausted) = match &self.strategy.termination {
            TerminationStrategy::Goal {
                criteria,
                max_attempts,
                on_exhausted,
            } => (criteria.clone(), *max_attempts, on_exhausted.clone()),
            _ => unreachable!(),
        };

        let mut last_eval: Option<CriteriaEvaluation> = None;

        for attempt in 0..max_attempts {
            self.apply_context_strategy_before_run().await?;

            let prompt = if attempt == 0 {
                build_initial_goal_prompt(&task, &criteria)
            } else {
                build_retry_prompt(&task, &criteria, attempt, max_attempts, last_eval.as_ref())
            };

            let result = self
                .prompt_with_content(vec![Content::Text {
                    text: prompt,
                    text_signature: None,
                }])
                .await?;

            let eval = evaluate_criteria(&result, &criteria);
            if eval.all_passed() {
                return Ok(GoalOutcome::Passed {
                    messages: result,
                    attempts: attempt + 1,
                });
            }
            last_eval = Some(eval);
        }

        match on_exhausted {
            GoalExhaustedAction::Abort => Err(AgentError::GoalNotMet {
                criteria: criteria.iter().map(|c| c.id.clone()).collect(),
                attempts: max_attempts,
            }),
            GoalExhaustedAction::ReturnLast => {
                let result = self
                    .prompt_with_content(vec![Content::Text {
                        text: task,
                        text_signature: None,
                    }])
                    .await?;
                Ok(GoalOutcome::Exhausted {
                    messages: result,
                    attempts: max_attempts,
                })
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // Strategy: Loop — background execution
    // ═══════════════════════════════════════════════════════════════

    fn spawn_background_loop(&self, task: String, delay: std::time::Duration, max: Option<u32>) {
        let abort = self.state_machine.abort_token();
        let event_tx = self.event_hub.event_tx_clone();
        let termination = self.strategy.termination.clone();
        let context = self.strategy.context.clone();
        let provider = self.provider.clone();
        let hook_dispatcher = self.hook_dispatcher.clone();
        let tools = self.tools.clone();
        let skills = self.skills.clone();
        let model = self.model.clone();
        let stream_options = self.stream_options.clone();
        let base_persona = self.base_persona.clone();
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();

        tokio::spawn(async move {
            let mut iteration: u32 = 1;

            loop {
                if abort.is_cancelled() {
                    break;
                }
                if let Some(max) = max
                    && iteration >= max
                {
                    break;
                }

                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = abort.cancelled() => { break; }
                }
                if abort.is_cancelled() {
                    break;
                }

                iteration += 1;

                let result = Self::run_background_iteration(
                    &task,
                    &termination,
                    &context,
                    &provider,
                    &hook_dispatcher,
                    &tools,
                    &skills,
                    &model,
                    &stream_options,
                    &base_persona,
                    &tenant_id,
                    &session_id,
                    &abort,
                )
                .await;

                match result {
                    Ok(msgs) => {
                        if let Some(tx) = &event_tx {
                            let _ = tx.try_send(event_hub::QueuedEvent {
                                event: AgentEvent::LoopIterationComplete {
                                    iteration,
                                    messages: msgs,
                                },
                            });
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            iteration,
                            error = %e,
                            "background loop iteration failed, continuing"
                        );
                        if let Some(tx) = &event_tx {
                            let _ = tx.try_send(event_hub::QueuedEvent {
                                event: AgentEvent::LoopIterationError {
                                    iteration,
                                    error: e.to_string(),
                                },
                            });
                        }
                    }
                }
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_background_iteration(
        task: &str,
        termination: &TerminationStrategy,
        context: &ContextStrategy,
        provider: &Arc<dyn ai_provider::LlmProvider>,
        hook_dispatcher: &Arc<dyn HookDispatcher>,
        tools: &[AgentToolRef],
        skills: &[crate::skills::Skill],
        model: &str,
        stream_options: &ai_provider::StreamOptions,
        base_persona: &str,
        tenant_id: &str,
        session_id: &str,
        abort: &CancellationToken,
    ) -> Result<Vec<AgentMessage>, AgentError> {
        let builder = match context {
            ContextStrategy::Clear => {
                let mut b = PromptBuilder::from_base(base_persona);
                crate::skills::inject_skills_into_builder(&mut b, skills);
                b
            }
            _ => PromptBuilder::default(),
        };

        let task_msg = if let TerminationStrategy::Goal { criteria, .. } = termination {
            build_initial_goal_prompt(task, criteria)
        } else {
            task.to_string()
        };

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: task_msg,
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let config = AgentLoopConfig {
            tenant_id: tenant_id.to_string(),
            session_id: session_id.to_string(),
            model: model.to_string(),
            provider: provider.clone(),
            hook_dispatcher: hook_dispatcher.clone(),
            tools: tools.to_vec(),
            prompt_builder: builder,
            stream_options: stream_options.clone(),
            steer_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            event_sink: Arc::new(|_| {}),
            circuit_breaker: None,
            skills: skills.to_vec(),
            text_stream_tx: None,
        };

        AgentLoop::new(config)
            .run(vec![user_msg], abort.child_token())
            .await
    }

    // ═══════════════════════════════════════════════════════════════
    // Strategy: Context
    // ═══════════════════════════════════════════════════════════════

    async fn apply_context_strategy_before_run(&mut self) -> Result<(), AgentError> {
        match &self.strategy.context {
            ContextStrategy::Accumulate => {}
            ContextStrategy::Compact { keep_last_n } => {
                if self.history.entries().len() > *keep_last_n {
                    let split_at = self.history.entries().len() - *keep_last_n;
                    let old: Vec<_> = self.history.entries().to_vec();
                    self.history.clear_entries();
                    let kept_tail: Vec<_> = old.iter().skip(split_at).cloned().collect();
                    for entry in kept_tail {
                        self.history.append_compaction_entry(entry);
                    }
                    let summary = self
                        .compaction_actor
                        .compact(&old, &self.state_machine.child_token())
                        .await
                        .map(|r| r.summary)
                        .unwrap_or_else(|e| {
                            tracing::warn!(error = %e, "context strategy compact failed");
                            "(compaction failed)".to_string()
                        });
                    self.prompt_builder.upsert_fragment(PromptFragment {
                        id: "strategy-compaction".into(),
                        kind: FragmentKind::RuntimeInjection,
                        source: FragmentSource::System,
                        content: format!("## Prior context (compacted)\n\n{summary}"),
                        priority: 150,
                    });
                }
            }
            ContextStrategy::Clear => {
                self.history.clear_entries();
                self.prompt_builder = PromptBuilder::from_base(self.base_persona.clone());
                crate::skills::inject_skills_into_builder(&mut self.prompt_builder, &self.skills);
            }
        }
        Ok(())
    }

    /// Persist the session lifecycle status to the configured store.
    ///
    /// No-op if no store is configured. Fire-and-forget — failures are
    /// logged but never block the agent loop.
    fn persist_status(&self, status: &str) {
        self.history.persist_status(status);
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
        self.state_machine.abort();

        // 2. Wait for in-flight persistence to complete
        if let Some(handle) = self.history.take_last_save() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }

        // 3. Drop event sender to signal the processor to exit
        self.event_hub.take_event_tx();

        // 4. Wait for the event processor with a timeout
        if let Some(handle) = self.event_hub.shutdown_handle() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), handle).await;
        }

        info!(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            "session shutdown complete",
        );
    }

    pub fn messages(&self) -> Vec<AgentMessage> {
        self.history.messages()
    }

    /// Return the full session history including compaction entries.
    pub fn entries(&self) -> &[SessionEntry] {
        self.history.entries()
    }

    /// Get the tenant ID
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// Get the session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Returns the token usage from the most recent agent loop turn, if any.
    pub fn last_usage(&self) -> Option<&ai_provider::Usage> {
        self.last_usage.as_ref()
    }

    /// Return the tools registered for this session.
    pub fn tools(&self) -> &[crate::types::AgentToolRef] {
        &self.tools
    }

    /// Access the session's message history subsystem.
    pub fn history(&self) -> &history::SessionHistory {
        &self.history
    }

    /// Access the session's event hub subsystem.
    pub fn event_hub(&self) -> &event_hub::SessionEventHub {
        &self.event_hub
    }

    /// Access the session's state machine subsystem.
    pub fn state_machine(&self) -> &state::SessionStateMachine {
        &self.state_machine
    }
}

#[cfg(any(test, feature = "testing"))]
impl SessionActor {
    /// Create a minimal, non-functional SessionActor for use in unit tests
    /// of downstream crates (e.g., session cache tests in tavern-comp).
    /// The returned actor cannot execute prompts — it exists solely as a
    /// placeholder for cache data structure tests.
    pub fn dummy_for_test() -> Self {
        use crate::harness::compaction::CompactionConfig;
        use crate::harness::session::SessionConfig;
        use crate::hook::default_dispatcher::DefaultHookDispatcher;
        use crate::space::AgentSpace;
        use std::sync::Arc;

        let dispatcher = Arc::new(DefaultHookDispatcher::from_config(
            AgentSpace::default(),
            &crate::harness::config::HookConfig::default(),
        ));
        let compaction = Arc::new(crate::harness::compaction::Compactor::new(
            CompactionConfig::default(),
            Arc::new(ai_provider::RouterProvider::new()),
            "dummy".into(),
            Arc::new(crate::file_ops::DefaultFileOperationExtractor::default()),
        ));

        Self::new(SessionConfig {
            tenant_id: "dummy".into(),
            session_id: "dummy".into(),
            system_prompt: String::new(),
            model: "dummy".into(),
            provider: Arc::new(ai_provider::RouterProvider::new()),
            hook_dispatcher: dispatcher,
            compaction_actor: compaction,
            tools: vec![],
            store: None,
            skills: vec![],
        })
    }
}

impl Drop for SessionActor {
    fn drop(&mut self) {
        // 1. Cancel any in-flight operations
        self.state_machine.abort();

        // 2. Drop event sender so the event processor sees channel closed
        //    and drains any buffered events before exiting naturally.
        //    We do NOT abort the handle here — a forced abort would drop
        //    events already sitting in the mpsc buffer (e.g. TurnEnd).
        self.event_hub.take_event_tx();

        // Take the handle out so it is dropped alongside this SessionActor.
        // JoinHandle::drop does NOT abort the task; the task continues to
        // run until the recv loop observes the closed channel.
        let _ = self.event_hub.shutdown_handle();

        // NOTE: `last_save` is intentionally NOT awaited here because
        // `Drop` cannot be async. Callers MUST call `shutdown()` (which
        // awaits `last_save` with a 5s timeout) before dropping the
        // `SessionActor` to guarantee all persistence writes complete.
        // A bare `drop()` will abort the in-flight save task, potentially
        // losing the last write.
    }
}

// ═══════════════════════════════════════════════════════════════════
// Goal prompt builders (free functions)
// ═══════════════════════════════════════════════════════════════════

fn build_initial_goal_prompt(task: &str, criteria: &[GoalCriterion]) -> String {
    let mut p = format!("## Task\n\n{task}\n\n## Acceptance Criteria\n");
    p.push_str("You must satisfy ALL of the following before responding:\n\n");
    for (i, c) in criteria.iter().enumerate() {
        p.push_str(&format!("{}. [{}] {}\n", i + 1, c.id, c.description));
    }
    p.push_str("\nAfter completing the task, end your response with a criteria checklist:\n\n");
    for c in criteria {
        p.push_str(&format!("[CRITERION_RESULT: {}: PASS|FAIL]\n", c.id));
    }
    p
}

fn build_retry_prompt(
    task: &str,
    criteria: &[GoalCriterion],
    attempt: u32,
    max_attempts: u32,
    last_eval: Option<&CriteriaEvaluation>,
) -> String {
    let mut p = format!(
        "## Acceptance Criteria Check (attempt {}/{})\n\n",
        attempt + 1,
        max_attempts
    );
    p.push_str("The previous response did not meet all criteria:\n\n");
    if let Some(eval) = last_eval {
        for (id, passed) in &eval.results {
            let mark = if *passed { "✓" } else { "✗" };
            let desc = criteria
                .iter()
                .find(|c| &c.id == id)
                .map(|c| c.description.as_str())
                .unwrap_or(id);
            p.push_str(&format!("{mark} {id} — {desc}\n"));
        }
    }
    p.push_str(&format!("\n## Original Task\n\n{task}\n\n"));
    p.push_str("Please fix the failing criteria and respond with the corrected implementation.\n");
    p.push_str("End with [CRITERION_RESULT: ...] as before.\n");
    p
}

/// Evaluate all criteria against the agent response.
///
/// `Command` and `OutputContains` verifications are run by the framework.
/// `SelfAssessment` parses the `[CRITERION_RESULT: id: PASS|FAIL]` markers
/// from the agent's output.
fn evaluate_criteria(messages: &[AgentMessage], criteria: &[GoalCriterion]) -> CriteriaEvaluation {
    let mut results = Vec::new();
    // Extract the assistant's text from the last assistant message
    let assistant_text = messages
        .iter()
        .rev()
        .find_map(|m| match m {
            AgentMessage::Assistant(a) => {
                let mut text = String::new();
                for c in &a.content {
                    if let Content::Text { text: t, .. } = c {
                        text.push_str(t);
                    }
                }
                Some(text)
            }
            _ => None,
        })
        .unwrap_or_default();

    for c in criteria {
        let passed = match &c.verification {
            GoalVerification::SelfAssessment => {
                parse_criterion_result(&assistant_text, &c.id).unwrap_or(false)
            }
            GoalVerification::Command { .. } => {
                // Commands are run synchronously — for now, mark as
                // pending (not evaluated at framework level).
                // In production, this would invoke tokio::process::Command.
                false
            }
            GoalVerification::OutputContains { text } => assistant_text.contains(text.as_str()),
        };
        results.push((c.id.clone(), passed));
    }
    CriteriaEvaluation { results }
}

fn parse_criterion_result(text: &str, id: &str) -> Option<bool> {
    let marker = format!("[CRITERION_RESULT: {id}:");
    // Find the marker line and extract PASS or FAIL
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix(&marker) {
            let rest = rest.trim_end_matches(']');
            return match rest.trim() {
                "PASS" => Some(true),
                "FAIL" => Some(false),
                _ => None,
            };
        }
    }
    None
}


#[cfg(test)]
mod tests;
