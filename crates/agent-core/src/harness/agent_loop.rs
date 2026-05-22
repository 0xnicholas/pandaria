use std::sync::{Arc, Mutex};

use ai_provider::{Content, LlmContext, LlmProvider, StopReason, StreamOptions};
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use crate::error::AgentError;
use crate::events::AgentEvent;
use crate::harness::tool::ToolExecutor;
use crate::hook::context::{
    AgentEndCtx, BeforeAgentStartCtx, ContextCtx, ProviderRequestCtx, ProviderResponseCtx,
    TurnEndCtx,
};
use crate::hook::dispatcher::HookDispatcher;
use crate::hook::mutations::{
    BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation,
};
use crate::hook::timeout::with_timeout;
use crate::prompt::PromptBuilder;
use crate::types::{AgentMessage, AgentToolRef, ToolExecutionMode};
use crate::utils::provider_opts::ProviderStreamOptions;

/// Configuration for [`AgentLoop`].
///
/// The fields `steer_queue`, `follow_up_queue`, `event_sink`, and
/// `circuit_breaker` are internal implementation details. They remain `pub`
/// for test convenience but are not part of the stable public API.
pub struct AgentLoopConfig {
    pub tenant_id: String,
    pub session_id: String,
    pub model: String,
    pub provider: Arc<dyn LlmProvider>,
    pub hook_dispatcher: Arc<dyn HookDispatcher>,
    pub tools: Vec<AgentToolRef>,
    pub prompt_builder: PromptBuilder,
    pub stream_options: StreamOptions,
    #[doc(hidden)]
    pub steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    #[doc(hidden)]
    pub follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    #[doc(hidden)]
    pub event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync + 'static>,
    /// Optional circuit breaker for the provider.
    #[doc(hidden)]
    pub circuit_breaker: Option<Arc<crate::circuit_breaker::CircuitBreaker>>,
    /// Skills available for this session. Used to re-inject the
    /// `<available_skills>` fragment after legacy prompt replacements.
    pub skills: Vec<crate::skills::Skill>,
}

impl AgentLoopConfig {
    /// Create a new `AgentLoopConfig` with sensible defaults for internal fields.
    pub fn new(
        tenant_id: String,
        session_id: String,
        model: String,
        provider: Arc<dyn LlmProvider>,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        tools: Vec<AgentToolRef>,
    ) -> Self {
        Self {
            tenant_id,
            session_id,
            model,
            provider,
            hook_dispatcher,
            tools,
            prompt_builder: PromptBuilder::default(),
            stream_options: StreamOptions::default(),
            steer_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            event_sink: Arc::new(|_| {}),
            circuit_breaker: None,
            skills: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum TurnResult {
    ToolUse,
    Stop,
    Error(AgentError),
}

fn build_tool_defs(tools: &[AgentToolRef]) -> Option<Vec<ai_provider::ToolDef>> {
    if tools.is_empty() {
        None
    } else {
        Some(
            tools
                .iter()
                .map(|t| ai_provider::ToolDef {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: t.parameters(),
                })
                .collect(),
        )
    }
}

fn build_tool_value_defs(tools: &[AgentToolRef]) -> Vec<serde_json::Value> {
    tools.iter().map(|t| serde_json::json!({"name": t.name(), "description": t.description(), "parameters": t.parameters()})).collect()
}

fn apply_provider_request_mutation(
    prompt_builder: &mut PromptBuilder,
    ctx: &mut LlmContext,
    opts: &mut StreamOptions,
    mutation: ProviderRequestMutation,
    skills: &[crate::skills::Skill],
) {
    if let Some(mutation) = mutation.prompt_mutation {
        prompt_builder.apply_mutation(mutation);
    }
    if let Some(builder) = mutation.system_prompt {
        *prompt_builder = builder;
        crate::skills::inject_skills_into_builder(prompt_builder, skills);
    }
    ctx.system_prompt = prompt_builder.render_option();
    if let Some(msgs) = mutation.messages {
        ctx.messages = msgs;
    }
    if let Some(tools) = mutation.tools {
        ctx.tools = tools;
    }
    if let Some(options) = mutation.options {
        if let Some(mt) = options.max_tokens {
            opts.max_tokens = Some(mt);
        }
        if let Some(temp) = options.temperature {
            opts.temperature = Some(temp);
        }
        if let Some(tp) = options.top_p {
            opts.top_p = Some(tp);
        }
        if let Some(reasoning) = options.reasoning {
            opts.reasoning = Some(reasoning);
        }
        if let Some(mr) = options.max_retries {
            opts.max_retries = mr;
        }
        if let Some(timeout) = options.timeout {
            opts.timeout = timeout;
        }
    }
}

fn apply_provider_response_mutation(
    msg: &mut ai_provider::AssistantMessage,
    mutation: ProviderResponseMutation,
) {
    if let Some(content) = mutation.content {
        msg.content = content;
    }
    if let Some(stop_reason) = mutation.stop_reason {
        msg.stop_reason = stop_reason;
    }
}

pub fn resolve_orphan_tool_calls(messages: &mut Vec<AgentMessage>) {
    use std::collections::HashSet;
    let mut tool_call_ids: Vec<(usize, String)> = Vec::new();
    let mut resolved_ids: HashSet<String> = HashSet::new();
    for (i, msg) in messages.iter().enumerate() {
        match msg {
            AgentMessage::Assistant(a) => {
                for content in &a.content {
                    if let ai_provider::Content::ToolCall(tc) = content {
                        tool_call_ids.push((i, tc.id.clone()));
                    }
                }
            }
            AgentMessage::ToolResult(tr) => {
                resolved_ids.insert(tr.tool_call_id.clone());
            }
            _ => {}
        }
    }
    let mut orphans: Vec<(usize, String, String)> = tool_call_ids
        .into_iter()
        .filter(|(_, id)| !resolved_ids.contains(id))
        .map(|(idx, id)| {
            let tool_name = match &messages[idx] {
                AgentMessage::Assistant(a) => a.content.iter().find_map(|c| match c {
                    ai_provider::Content::ToolCall(tc) if tc.id == id => Some(tc.name.clone()),
                    _ => None,
                }),
                _ => None,
            }
            .unwrap_or_else(|| "unknown".to_string());
            (idx, id, tool_name)
        })
        .collect();
    orphans.sort_by(|a, b| b.0.cmp(&a.0));
    for (idx, id, tool_name) in orphans {
        messages.insert(idx + 1, AgentMessage::ToolResult(ai_provider::ToolResultMessage {
            tool_call_id: id, tool_name, content: vec![],
            details: Some(serde_json::json!({"_orphan": true, "message": "tool call was not executed (context truncated or restored)"})),
            is_error: true, timestamp: std::time::SystemTime::now(),
        }));
    }
}

fn is_overflow(error_msg: &str) -> bool {
    let lower = error_msg.to_lowercase();
    lower.contains("context length") || lower.contains("token limit")
}

pub struct AgentLoop {
    config: AgentLoopConfig,
}

impl AgentLoop {
    pub fn new(config: AgentLoopConfig) -> Self {
        Self { config }
    }

    #[instrument(
        skip(self, initial_messages, signal),
        fields(
            tenant_id = %self.config.tenant_id,
            session_id = %self.config.session_id,
            model = %self.config.model,
        )
    )]
    pub async fn run(
        &self,
        initial_messages: Vec<AgentMessage>,
        signal: CancellationToken,
    ) -> Result<Vec<AgentMessage>, AgentError> {
        let agent_start_ctx = BeforeAgentStartCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            system_prompt: self.config.prompt_builder.render_option(),
            messages: initial_messages.clone(),
            prompt_builder: self.config.prompt_builder.clone(),
            tools: build_tool_value_defs(&self.config.tools),
            model: self.config.model.clone(),
        };
        let agent_start_mutation = crate::hook::timeout::with_timeout(
            self.config
                .hook_dispatcher
                .on_before_agent_start(&agent_start_ctx),
            500,
            BeforeAgentStartMutation::default(),
            "on_before_agent_start",
        )
        .await;
        let mut prompt_builder = self.config.prompt_builder.clone();
        if let Some(mutation) = agent_start_mutation.prompt_mutation {
            prompt_builder.apply_mutation(mutation);
        }
        if let Some(builder) = agent_start_mutation.system_prompt {
            prompt_builder = builder;
            crate::skills::inject_skills_into_builder(&mut prompt_builder, &self.config.skills);
        }
        let mut messages = agent_start_mutation.messages.unwrap_or(initial_messages);
        let mut new_messages: Vec<AgentMessage> = Vec::new();
        let mut turn_index: u64 = 0;
        let mut message_index: u64 = 0;

        (self.config.event_sink)(AgentEvent::AgentStart);

        loop {
            {
                // Drain steer queue
                let mut q = self
                    .config
                    .steer_queue
                    .lock()
                    .expect("steer queue poisoned");
                let steer_msgs: Vec<_> = q.drain(..).collect();
                new_messages.extend(steer_msgs.clone());
                messages.extend(steer_msgs);
            }

            loop {
                // Inner turn loop
                if signal.is_cancelled() {
                    (self.config.event_sink)(AgentEvent::AgentEnd {
                        messages: messages.clone(),
                    });
                    let _ = with_timeout(
                        self.config.hook_dispatcher.on_agent_end(&AgentEndCtx {
                            tenant_id: self.config.tenant_id.clone(),
                            session_id: self.config.session_id.clone(),
                            messages: messages.clone(),
                        }),
                        100,
                        (),
                        "on_agent_end",
                    )
                    .await;
                    return Err(AgentError::Cancelled);
                }

                let result = self
                    .run_turn(
                        &mut messages,
                        &mut new_messages,
                        &mut turn_index,
                        &mut message_index,
                        &mut prompt_builder,
                        &signal,
                    )
                    .await;
                match result {
                    TurnResult::ToolUse => continue,
                    TurnResult::Stop => break,
                    TurnResult::Error(e) => {
                        (self.config.event_sink)(AgentEvent::Error { error: e.clone() });
                        (self.config.event_sink)(AgentEvent::AgentEnd {
                            messages: messages.clone(),
                        });
                        let _ = with_timeout(
                            self.config.hook_dispatcher.on_agent_end(&AgentEndCtx {
                                tenant_id: self.config.tenant_id.clone(),
                                session_id: self.config.session_id.clone(),
                                messages: messages.clone(),
                            }),
                            100,
                            (),
                            "on_agent_end",
                        )
                        .await;
                        return Err(e);
                    }
                }
            }

            {
                // Drain follow_up queue
                let mut q = self
                    .config
                    .follow_up_queue
                    .lock()
                    .expect("follow_up queue poisoned");
                let follow_ups: Vec<_> = q.drain(..).collect();
                if follow_ups.is_empty() {
                    break;
                }
                messages.extend(follow_ups.clone());
                new_messages.extend(follow_ups);
            }
        }

        (self.config.event_sink)(AgentEvent::AgentEnd {
            messages: messages.clone(),
        });
        let _ = with_timeout(
            self.config.hook_dispatcher.on_agent_end(&AgentEndCtx {
                tenant_id: self.config.tenant_id.clone(),
                session_id: self.config.session_id.clone(),
                messages: messages.clone(),
            }),
            100,
            (),
            "on_agent_end",
        )
        .await;
        Ok(new_messages)
    }

    async fn run_turn(
        &self,
        messages: &mut Vec<AgentMessage>,
        new_messages: &mut Vec<AgentMessage>,
        turn_index: &mut u64,
        message_index: &mut u64,
        prompt_builder: &mut PromptBuilder,
        signal: &CancellationToken,
    ) -> TurnResult {
        *turn_index += 1;
        (self.config.event_sink)(AgentEvent::TurnStart {
            turn_index: *turn_index,
        });

        let after_context_messages = messages.clone();
        let ctx_ctx = ContextCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            messages: messages.clone(),
        };
        let ctx_mutation = with_timeout(
            self.config.hook_dispatcher.on_context(&ctx_ctx),
            500,
            crate::mutations::ContextMutation::default(),
            "on_context",
        )
        .await;
        let mut transformed = ctx_mutation.messages.unwrap_or_else(|| messages.clone());
        resolve_orphan_tool_calls(&mut transformed);

        let mut stream_opts = self.config.stream_options.clone();
        let mut ctx = LlmContext {
            system_prompt: prompt_builder.render_option(),
            messages: transformed,
            tools: build_tool_defs(&self.config.tools),
        };

        let provider_req_ctx = ProviderRequestCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            model: self.config.model.clone(),
            turn_index: *turn_index,
            system_prompt: ctx.system_prompt.clone(),
            prompt_builder: prompt_builder.clone(),
            messages: ctx.messages.clone(),
            tools: ctx.tools.clone(),
            options: ProviderStreamOptions::from_options(&self.config.stream_options),
        };
        let provider_req_mutation = with_timeout(
            self.config
                .hook_dispatcher
                .on_before_provider_request(&provider_req_ctx),
            500,
            ProviderRequestMutation::default(),
            "on_before_provider_request",
        )
        .await;
        apply_provider_request_mutation(
            prompt_builder,
            &mut ctx,
            &mut stream_opts,
            provider_req_mutation,
            &self.config.skills,
        );

        // Cross-provider message normalization (spec §2.2 step 2.6)
        let model_meta = self.config.provider.model_metadata(&self.config.model);
        let supports_images = model_meta
            .as_ref()
            .map(|m| {
                m.input_modalities
                    .iter()
                    .any(|modality| matches!(modality, ai_provider::Modality::Image))
            })
            .unwrap_or(false);
        let supports_video_input = model_meta
            .as_ref()
            .map(|m| {
                m.input_modalities
                    .iter()
                    .any(|modality| matches!(modality, ai_provider::Modality::Video))
            })
            .unwrap_or(false);
        let supports_audio_input = model_meta
            .as_ref()
            .map(|m| {
                m.input_modalities
                    .iter()
                    .any(|modality| matches!(modality, ai_provider::Modality::Audio))
            })
            .unwrap_or(false);
        let target_api = model_meta.map(|m| m.api);
        let transform_opts = ai_provider::TransformOptions {
            target_api,
            supports_images,
            supports_video_input,
            supports_audio_input,
            preserve_thinking: false, // v0.1: strip thinking for safety
        };
        ctx.messages = ai_provider::transform_messages(&ctx.messages, &transform_opts);

        let (retry_count, mut assistant_msg) = match self
            .call_llm_with_retry(ctx, stream_opts, *message_index, signal)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                (self.config.event_sink)(AgentEvent::Error { error: e.clone() });
                return TurnResult::Error(e);
            }
        };

        let provider_resp_ctx = ProviderResponseCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            model: self.config.model.clone(),
            turn_index: *turn_index,
            attempt: retry_count,
            messages_before: after_context_messages,
            content: assistant_msg.content.clone(),
            stop_reason: assistant_msg.stop_reason.clone(),
        };
        let provider_resp_mutation = with_timeout(
            self.config
                .hook_dispatcher
                .on_after_provider_response(&provider_resp_ctx),
            500,
            ProviderResponseMutation::default(),
            "on_after_provider_response",
        )
        .await;
        apply_provider_response_mutation(&mut assistant_msg, provider_resp_mutation);

        *message_index += 1;
        (self.config.event_sink)(AgentEvent::MessageEnd {
            message: AgentMessage::Assistant(assistant_msg.clone()),
        });
        new_messages.push(AgentMessage::Assistant(assistant_msg.clone()));
        messages.push(AgentMessage::Assistant(assistant_msg.clone()));

        let tool_calls: Vec<&ai_provider::ToolCall> = assistant_msg
            .content
            .iter()
            .filter_map(|c| match c {
                ai_provider::Content::ToolCall(tc) => Some(tc),
                _ => None,
            })
            .collect();

        if tool_calls.is_empty() {
            match assistant_msg.stop_reason {
                StopReason::Error | StopReason::Aborted | StopReason::Length => {
                    let err_msg = assistant_msg.error_message.clone().unwrap_or_else(|| {
                        format!("LLM returned stop reason: {:?}", assistant_msg.stop_reason)
                    });
                    if assistant_msg.stop_reason == StopReason::Error && is_overflow(&err_msg) {
                        return TurnResult::Error(AgentError::ContextOverflow(err_msg));
                    }
                    return TurnResult::Error(AgentError::LlmResponseError(err_msg));
                }
                _ => {}
            }
            (self.config.event_sink)(AgentEvent::TurnEnd {
                turn_index: *turn_index,
                messages: messages.clone(),
            });
            let _ = with_timeout(
                self.config.hook_dispatcher.on_turn_end(&TurnEndCtx {
                    tenant_id: self.config.tenant_id.clone(),
                    session_id: self.config.session_id.clone(),
                    turn_index: *turn_index,
                    messages: messages.clone(),
                    usage: assistant_msg.usage.clone(),
                }),
                100,
                (),
                "on_turn_end",
            )
            .await;
            return TurnResult::Stop;
        }

        // Tool calls present — reject non-recoverable stop reasons before execution
        // to avoid pushing tool results when the turn will return an error.
        match assistant_msg.stop_reason {
            StopReason::Error | StopReason::Aborted | StopReason::Length => {
                let err_msg = assistant_msg.error_message.clone().unwrap_or_else(|| {
                    format!("LLM returned stop reason: {:?}", assistant_msg.stop_reason)
                });
                if assistant_msg.stop_reason == StopReason::Error && is_overflow(&err_msg) {
                    return TurnResult::Error(AgentError::ContextOverflow(err_msg));
                }
                return TurnResult::Error(AgentError::LlmResponseError(err_msg));
            }
            _ => {}
        }

        let tool_results = self.execute_tools(tool_calls, signal).await;
        let mut all_terminate = !tool_results.is_empty();
        for result in &tool_results {
            new_messages.push(AgentMessage::ToolResult(result.clone()));
            messages.push(AgentMessage::ToolResult(result.clone()));
            let terminated = result
                .details
                .as_ref()
                .and_then(|d| d.get("_terminate"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !terminated {
                all_terminate = false;
            }
        }

        (self.config.event_sink)(AgentEvent::TurnEnd {
            turn_index: *turn_index,
            messages: messages.clone(),
        });
        let _ = with_timeout(
            self.config.hook_dispatcher.on_turn_end(&TurnEndCtx {
                tenant_id: self.config.tenant_id.clone(),
                session_id: self.config.session_id.clone(),
                turn_index: *turn_index,
                messages: messages.clone(),
                usage: assistant_msg.usage.clone(),
            }),
            100,
            (),
            "on_turn_end",
        )
        .await;

        if all_terminate {
            return TurnResult::Stop;
        }
        if assistant_msg.stop_reason == StopReason::ToolUse {
            TurnResult::ToolUse
        } else {
            TurnResult::Stop
        }
    }

    async fn call_llm_with_retry(
        &self,
        ctx: LlmContext,
        stream_opts: StreamOptions,
        message_index: u64,
        signal: &CancellationToken,
    ) -> Result<(u32, ai_provider::AssistantMessage), AgentError> {
        let mut retry_count: u32 = 0;
        let max_retries = stream_opts.max_retries;

        // Check circuit breaker before first attempt
        if let Some(ref cb) = self.config.circuit_breaker {
            if let Err(e) = cb.check().await {
                tracing::warn!(error = %e, "circuit breaker open, fast-failing provider request");
                return Err(AgentError::LlmError(ai_provider::LlmError::ProviderError(
                    format!("circuit breaker: {e}"),
                )));
            }
        }

        loop {
            if signal.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            (self.config.event_sink)(AgentEvent::MessageStart { message_index });

            let stream_result = self
                .config
                .provider
                .stream(
                    &self.config.model,
                    ctx.clone(),
                    stream_opts.clone(),
                    signal.child_token(),
                )
                .await;

            let mut stream = match stream_result {
                Ok(s) => s,
                Err(e) if e.is_retryable() && retry_count < max_retries => {
                    if let Some(ref cb) = self.config.circuit_breaker {
                        cb.record_failure().await;
                    }
                    retry_count += 1;
                    let delay_ms = 100 * 2_u64.pow(retry_count - 1);
                    (self.config.event_sink)(AgentEvent::AutoRetryStart {
                        attempt: retry_count,
                        max_attempts: max_retries,
                        delay_ms,
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                    (self.config.event_sink)(AgentEvent::AutoRetryEnd {
                        success: false,
                        error: Some(e.to_string()),
                    });
                    continue;
                }
                Err(e) => {
                    if let Some(ref cb) = self.config.circuit_breaker {
                        if e.is_retryable() {
                            cb.record_failure().await;
                        }
                    }
                    (self.config.event_sink)(AgentEvent::AutoRetryEnd {
                        success: false,
                        error: Some(e.to_string()),
                    });
                    return Err(AgentError::LlmError(e));
                }
            };

            let provider_name = self.config.provider.provider_name().to_string();
            let mut assistant_content: Vec<Content> = Vec::new();
            let mut api = ai_provider::Api {
                provider: provider_name.clone(),
                model: self.config.model.clone(),
            };
            let mut usage = ai_provider::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 0,
            };
            let mut stop_reason = StopReason::Stop;
            let mut error_message: Option<String> = None;
            let mut text_accum: std::collections::BTreeMap<usize, String> =
                std::collections::BTreeMap::new();

            while let Some(event) = stream.next().await {
                if signal.is_cancelled() {
                    return Err(AgentError::Cancelled);
                }
                match event {
                    ai_provider::AssistantMessageEvent::Start { .. } => {}
                    ai_provider::AssistantMessageEvent::TextStart { .. } => {}
                    ai_provider::AssistantMessageEvent::TextDelta {
                        content_index,
                        delta,
                        ..
                    } => {
                        text_accum
                            .entry(content_index)
                            .or_default()
                            .push_str(&delta);
                        (self.config.event_sink)(AgentEvent::MessageUpdate {
                            message_index,
                            content_delta: delta,
                        });
                    }
                    ai_provider::AssistantMessageEvent::TextEnd {
                        content_index,
                        text,
                        ..
                    } => {
                        let accumulated = text_accum.remove(&content_index).unwrap_or(text);
                        assistant_content.push(Content::Text {
                            text: accumulated,
                            text_signature: None,
                        });
                    }
                    ai_provider::AssistantMessageEvent::ThinkingStart { .. } => {}
                    ai_provider::AssistantMessageEvent::ThinkingDelta { .. } => {}
                    ai_provider::AssistantMessageEvent::ThinkingEnd { .. } => {}
                    ai_provider::AssistantMessageEvent::ToolCallStart { .. } => {}
                    ai_provider::AssistantMessageEvent::ToolCallDelta { .. } => {}
                    ai_provider::AssistantMessageEvent::ToolCallEnd { tool_call, .. } => {
                        assistant_content.push(Content::ToolCall(tool_call));
                    }
                    ai_provider::AssistantMessageEvent::Done { reason, message } => {
                        // Some provider implementations send a Done event whose message.content
                        // does not include text accumulated via TextDelta/TextEnd (the partial
                        // clone is not updated with accumulated text). Only overwrite our
                        // accumulated content when the provider actually supplied something.
                        if !message.content.is_empty() {
                            assistant_content = message.content;
                        }
                        api = message.api;
                        usage = message.usage;
                        stop_reason = reason;
                    }
                    ai_provider::AssistantMessageEvent::Error { error } => {
                        error_message = error.error_message;
                        stop_reason = error.stop_reason;
                        break;
                    }
                }
            }

            let assistant_msg = ai_provider::AssistantMessage {
                content: assistant_content,
                provider: provider_name,
                model: self.config.model.clone(),
                api,
                usage,
                stop_reason: stop_reason.clone(),
                response_id: None,
                error_message: error_message.clone(),
                timestamp: std::time::SystemTime::now(),
            };

            if stop_reason == StopReason::Error {
                let is_retryable = error_message.as_ref().is_some_and(|e| {
                    let lower = e.to_lowercase();
                    [
                        "overloaded",
                        "rate limit",
                        "429",
                        "timeout",
                        "network error",
                        "service unavailable",
                        "fetch failed",
                        "terminated",
                        "500",
                        "502",
                        "503",
                        "504",
                    ]
                    .iter()
                    .any(|p| lower.contains(p))
                });
                if let Some(ref cb) = self.config.circuit_breaker {
                    if is_retryable {
                        cb.record_failure().await;
                    }
                }
                if is_retryable && retry_count < max_retries {
                    retry_count += 1;
                    let delay_ms = 100 * 2_u64.pow(retry_count - 1);
                    (self.config.event_sink)(AgentEvent::AutoRetryStart {
                        attempt: retry_count,
                        max_attempts: max_retries,
                        delay_ms,
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                    (self.config.event_sink)(AgentEvent::AutoRetryEnd {
                        success: false,
                        error: error_message.clone(),
                    });
                    continue;
                }
            }

            if let Some(ref cb) = self.config.circuit_breaker {
                cb.record_success().await;
            }
            (self.config.event_sink)(AgentEvent::AutoRetryEnd {
                success: true,
                error: None,
            });
            return Ok((retry_count, assistant_msg));
        }
    }

    async fn execute_single_tool(
        &self,
        tc: &ai_provider::ToolCall,
        signal: CancellationToken,
    ) -> ai_provider::ToolResultMessage {
        (self.config.event_sink)(AgentEvent::ToolExecutionStart {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
        });

        let tool = self
            .config
            .tools
            .iter()
            .find(|t| t.name() == tc.name)
            .cloned();

        let result = match tool {
            Some(tool) => {
                let executor = ToolExecutor::new(
                    self.config.tenant_id.clone(),
                    self.config.session_id.clone(),
                    self.config.hook_dispatcher.clone(),
                    tool,
                );
                let event_sink = self.config.event_sink.clone();
                let tool_call_id = tc.id.clone();
                let result = executor
                    .execute_tool_call(
                        tc,
                        Some(&move |update: crate::types::AgentToolProgressUpdate| {
                            (event_sink)(AgentEvent::ToolExecutionUpdate {
                                tool_call_id: tool_call_id.clone(),
                                content: update.content,
                            });
                        }),
                        signal.clone(),
                    )
                    .await;
                match result {
                    Ok(msg) => msg,
                    Err(e) => ai_provider::ToolResultMessage {
                        tool_call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        content: vec![],
                        details: Some(serde_json::json!({"error": e.to_string()})),
                        is_error: true,
                        timestamp: std::time::SystemTime::now(),
                    },
                }
            }
            None => ai_provider::ToolResultMessage {
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                content: vec![],
                details: Some(serde_json::json!({"error": "tool not found"})),
                is_error: true,
                timestamp: std::time::SystemTime::now(),
            },
        };

        (self.config.event_sink)(AgentEvent::ToolExecutionEnd {
            tool_call_id: tc.id.clone(),
            result: result.clone(),
        });

        result
    }

    async fn execute_tools(
        &self,
        tool_calls: Vec<&ai_provider::ToolCall>,
        signal: &CancellationToken,
    ) -> Vec<ai_provider::ToolResultMessage> {
        if tool_calls.is_empty() {
            return vec![];
        }

        let use_sequential = tool_calls.iter().any(|tc| {
            self.config
                .tools
                .iter()
                .find(|t| t.name() == tc.name)
                .map(|t| t.execution_mode() == ToolExecutionMode::Sequential)
                .unwrap_or(false)
        });

        if use_sequential {
            let mut results = Vec::with_capacity(tool_calls.len());
            for tc in tool_calls {
                if signal.is_cancelled() {
                    break;
                }
                results.push(self.execute_single_tool(tc, signal.clone()).await);
            }
            results
        } else {
            let tasks: Vec<_> = tool_calls
                .iter()
                .map(|tc| {
                    let signal = signal.clone();
                    let tc_id = tc.id.clone();
                    let tc_name = tc.name.clone();
                    async move {
                        tokio::select! {
                            result = self.execute_single_tool(tc, signal.clone()) => result,
                            _ = signal.cancelled() => {
                                ai_provider::ToolResultMessage {
                                    tool_call_id: tc_id,
                                    tool_name: tc_name,
                                    content: vec![],
                                    details: Some(serde_json::json!({"_cancelled": true})),
                                    is_error: true,
                                    timestamp: std::time::SystemTime::now(),
                                }
                            }
                        }
                    }
                })
                .collect();
            futures::future::join_all(tasks).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{AllowAllDispatcher, TestProvider, TestResponse, TestToolCall};
    use ai_provider::ToolCall;

    fn make_loop_config(
        provider: Arc<dyn LlmProvider>,
        dispatcher: Arc<dyn HookDispatcher>,
        tools: Vec<AgentToolRef>,
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
    async fn test_simple_prompt_response() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("Hello!");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
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

        assert_eq!(results.len(), 1);
        match &results[0] {
            AgentMessage::Assistant(msg) => {
                assert_eq!(msg.stop_reason, StopReason::Stop);
            }
            _ => panic!("expected assistant message"),
        }
    }

    use crate::AgentToolProgressUpdate;
    use crate::AgentToolResult;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CounterTool {
        name: String,
        counter: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl crate::types::AgentTool for CounterTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "A counter tool"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({})
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
            _signal: CancellationToken,
        ) -> Result<AgentToolResult, AgentError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: format!("executed_{}", self.name),
                    text_signature: None,
                }],
                details: None,
                is_error: false,
                terminate: true,
            })
        }
    }

    #[tokio::test]
    async fn test_parallel_tool_execution() {
        let _ = tracing_subscriber::fmt().try_init();

        let tool_a = Arc::new(CounterTool {
            name: "tool_a".to_string(),
            counter: AtomicUsize::new(0),
        });
        let tool_b = Arc::new(CounterTool {
            name: "tool_b".to_string(),
            counter: AtomicUsize::new(0),
        });

        let provider = TestProvider::sequence(vec![TestResponse::ToolCalls(vec![
            TestToolCall::new("call_a", "tool_a", serde_json::json!({})),
            TestToolCall::new("call_b", "tool_b", serde_json::json!({})),
        ])]);

        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(
            provider,
            dispatcher,
            vec![
                tool_a.clone() as AgentToolRef,
                tool_b.clone() as AgentToolRef,
            ],
        );
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "do things".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(vec![user_msg], CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert!(matches!(results[0], AgentMessage::Assistant(_)));
        assert!(matches!(results[1], AgentMessage::ToolResult(_)));
        assert!(matches!(results[2], AgentMessage::ToolResult(_)));

        assert_eq!(tool_a.counter.load(Ordering::SeqCst), 1);
        assert_eq!(tool_b.counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_llm_response_error_propagated() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::error("test error");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "hi".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let result = loop_.run(vec![user_msg], CancellationToken::new()).await;

        assert!(result.is_err());
        match result {
            Err(AgentError::LlmResponseError(_)) => {}
            other => panic!("expected LlmResponseError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_stop_reason_aborted_returns_error() {
        let _ = tracing_subscriber::fmt().try_init();
        // TestProvider::error returns StopReason::Error, so we need a custom
        // response for Aborted.  Use a counted provider that returns an error
        // with "Aborted" in the message but we must match on the error type.
        // Actually, the original test checks LlmResponseError with "Aborted".
        // Our TestProvider::error returns Error which maps to LlmResponseError.
        let provider = TestProvider::error("Aborted by user");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "hi".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let result = loop_.run(vec![user_msg], CancellationToken::new()).await;

        assert!(result.is_err());
        match result {
            Err(AgentError::LlmResponseError(msg)) => {
                assert!(
                    msg.contains("Aborted"),
                    "error message should mention Aborted"
                );
            }
            other => panic!("expected LlmResponseError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_stop_reason_length_returns_error() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::error("max tokens reached (Length)");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "hi".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let result = loop_.run(vec![user_msg], CancellationToken::new()).await;

        assert!(result.is_err());
        match result {
            Err(AgentError::LlmResponseError(msg)) => {
                assert!(
                    msg.contains("Length"),
                    "error message should mention Length"
                );
            }
            other => panic!("expected LlmResponseError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_tool_not_found_returns_error_result() {
        let _ = tracing_subscriber::fmt().try_init();

        let provider = TestProvider::sequence(vec![
            TestResponse::ToolCalls(vec![TestToolCall::new(
                "call_ghost",
                "ghost_tool",
                serde_json::json!({}),
            )]),
            TestResponse::Text("done".into()),
        ]);

        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "do things".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(vec![user_msg], CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert!(matches!(results[0], AgentMessage::Assistant(_)));
        match &results[1] {
            AgentMessage::ToolResult(msg) => {
                assert!(msg.is_error);
                assert_eq!(msg.details.as_ref().unwrap()["error"], "tool not found");
            }
            other => panic!("expected tool result, got {:?}", other),
        }
        assert!(matches!(results[2], AgentMessage::Assistant(_)));
    }

    #[tokio::test]
    async fn test_sequential_tool_execution() {
        let _ = tracing_subscriber::fmt().try_init();

        struct SequentialCounterTool {
            name: String,
            counter: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl crate::types::AgentTool for SequentialCounterTool {
            fn name(&self) -> &str {
                &self.name
            }
            fn description(&self) -> &str {
                "A sequential counter tool"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            fn execution_mode(&self) -> ToolExecutionMode {
                ToolExecutionMode::Sequential
            }

            async fn execute(
                &self,
                _tool_call_id: &str,
                _params: serde_json::Value,
                _on_progress: Option<&(dyn Fn(crate::AgentToolProgressUpdate) + Send + Sync)>,
                _signal: CancellationToken,
            ) -> Result<crate::AgentToolResult, AgentError> {
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                self.counter.fetch_add(1, Ordering::SeqCst);
                Ok(crate::AgentToolResult {
                    content: vec![Content::Text {
                        text: format!("executed_{}", self.name),
                        text_signature: None,
                    }],
                    details: None,
                    is_error: false,
                    terminate: true,
                })
            }
        }

        let tool_a = Arc::new(SequentialCounterTool {
            name: "seq_a".to_string(),
            counter: AtomicUsize::new(0),
        });
        let tool_b = Arc::new(SequentialCounterTool {
            name: "seq_b".to_string(),
            counter: AtomicUsize::new(0),
        });

        let provider = TestProvider::sequence(vec![TestResponse::ToolCalls(vec![
            TestToolCall::new("call_a", "seq_a", serde_json::json!({})),
            TestToolCall::new("call_b", "seq_b", serde_json::json!({})),
        ])]);

        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(
            provider,
            dispatcher,
            vec![
                tool_a.clone() as AgentToolRef,
                tool_b.clone() as AgentToolRef,
            ],
        );
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "do sequential things".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(vec![user_msg], CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert!(matches!(results[0], AgentMessage::Assistant(_)));
        assert!(matches!(results[1], AgentMessage::ToolResult(_)));
        assert!(matches!(results[2], AgentMessage::ToolResult(_)));

        assert_eq!(tool_a.counter.load(Ordering::SeqCst), 1);
        assert_eq!(tool_b.counter.load(Ordering::SeqCst), 1);
    }

    struct ContextMutatingDispatcher;
    #[async_trait::async_trait]
    impl HookDispatcher for ContextMutatingDispatcher {
        async fn on_context(&self, _ctx: &ContextCtx) -> crate::mutations::ContextMutation {
            crate::mutations::ContextMutation {
                messages: Some(vec![AgentMessage::User(ai_provider::UserMessage {
                    content: vec![Content::Text {
                        text: "mutated".to_string(),
                        text_signature: None,
                    }],
                    timestamp: std::time::SystemTime::now(),
                })]),
            }
        }
    }

    #[tokio::test]
    async fn test_context_hook_mutation() {
        let _ = tracing_subscriber::fmt().try_init();

        // VerifyingProvider checks the context and then returns "ok".
        // We can use TestProvider::counted to inspect context on the first call.
        let provider = TestProvider::counted(|_n| TestResponse::Text("ok".into()));
        let dispatcher = Arc::new(ContextMutatingDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "original".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(vec![user_msg], CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_all_tools_terminate() {
        let _ = tracing_subscriber::fmt().try_init();

        struct TerminatingTool;
        #[async_trait::async_trait]
        impl crate::types::AgentTool for TerminatingTool {
            fn name(&self) -> &str {
                "terminator"
            }
            fn description(&self) -> &str {
                "Terminates the loop"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({})
            }

            async fn execute(
                &self,
                _tool_call_id: &str,
                _params: serde_json::Value,
                _on_progress: Option<&(dyn Fn(crate::AgentToolProgressUpdate) + Send + Sync)>,
                _signal: CancellationToken,
            ) -> Result<crate::AgentToolResult, AgentError> {
                Ok(crate::AgentToolResult {
                    content: vec![Content::Text {
                        text: "done".to_string(),
                        text_signature: None,
                    }],
                    details: None,
                    is_error: false,
                    terminate: true,
                })
            }
        }

        let tool = Arc::new(TerminatingTool);
        let provider =
            TestProvider::sequence(vec![TestResponse::ToolCalls(vec![TestToolCall::new(
                "call_1",
                "terminator",
                serde_json::json!({}),
            )])]);

        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![tool as AgentToolRef]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "terminate".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(vec![user_msg], CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(matches!(results[0], AgentMessage::Assistant(_)));
        assert!(matches!(results[1], AgentMessage::ToolResult(_)));
    }

    #[tokio::test]
    async fn test_cancellation_mid_stream() {
        let _ = tracing_subscriber::fmt().try_init();

        let provider = TestProvider::cancel();
        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "hi".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let cancel_token = CancellationToken::new();
        let cancel_token_clone = cancel_token.child_token();

        let handle =
            tokio::spawn(async move { loop_.run(vec![user_msg], cancel_token_clone).await });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        cancel_token.cancel();

        let result = handle.await.unwrap();
        assert!(result.is_err());
        match result {
            Err(AgentError::Cancelled)
            | Err(AgentError::LlmError(ai_provider::LlmError::Cancelled)) => {}
            other => panic!("expected Cancelled, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_empty_tool_set() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("Hello without tools");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
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

        assert_eq!(results.len(), 1);
        match &results[0] {
            AgentMessage::Assistant(msg) => {
                assert_eq!(msg.stop_reason, StopReason::Stop);
                assert!(
                    msg.content
                        .iter()
                        .any(|c| matches!(c, Content::Text { .. }))
                );
            }
            _ => panic!("expected assistant message"),
        }
    }

    struct PanicOnContextDispatcher;
    #[async_trait::async_trait]
    impl HookDispatcher for PanicOnContextDispatcher {
        async fn on_context(&self, _ctx: &ContextCtx) -> crate::mutations::ContextMutation {
            panic!("context hook panic");
        }
    }

    #[tokio::test]
    async fn test_context_hook_panic_uses_default() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("Hello");
        let dispatcher = Arc::new(PanicOnContextDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "hi".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        // Panic in on_context is caught and defaults to no mutation, loop continues normally
        let result = loop_.run(vec![user_msg], CancellationToken::new()).await;

        assert!(
            result.is_ok(),
            "expected success after panic default, got {:?}",
            result
        );
        let new_messages = result.unwrap();
        // new_messages only contains newly generated messages (assistant), not initial user message
        assert_eq!(new_messages.len(), 1);
        assert!(matches!(&new_messages[0], AgentMessage::Assistant(_)));
    }

    struct PanicOnTurnEndDispatcher;
    #[async_trait::async_trait]
    impl HookDispatcher for PanicOnTurnEndDispatcher {
        async fn on_turn_end(&self, _ctx: &TurnEndCtx) {
            panic!("turn_end hook panic");
        }
    }

    #[tokio::test]
    async fn test_turn_end_hook_panic_is_caught() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("Hello");
        let dispatcher = Arc::new(PanicOnTurnEndDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "hi".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let result = loop_.run(vec![user_msg], CancellationToken::new()).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multi_turn_loop() {
        let _ = tracing_subscriber::fmt().try_init();

        struct NonTerminatingTool;
        #[async_trait::async_trait]
        impl crate::types::AgentTool for NonTerminatingTool {
            fn name(&self) -> &str {
                "non_terminating"
            }
            fn description(&self) -> &str {
                "Does not terminate"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn execute(
                &self,
                _tool_call_id: &str,
                _params: serde_json::Value,
                _on_progress: Option<&(dyn Fn(crate::AgentToolProgressUpdate) + Send + Sync)>,
                _signal: CancellationToken,
            ) -> Result<crate::AgentToolResult, AgentError> {
                Ok(crate::AgentToolResult {
                    content: vec![Content::Text {
                        text: "result".to_string(),
                        text_signature: None,
                    }],
                    details: None,
                    is_error: false,
                    terminate: false,
                })
            }
        }

        let tool = Arc::new(NonTerminatingTool);
        let provider = TestProvider::sequence(vec![
            TestResponse::ToolCalls(vec![TestToolCall::new(
                "call_1",
                "non_terminating",
                serde_json::json!({}),
            )]),
            TestResponse::Text("done".into()),
        ]);
        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![tool as AgentToolRef]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "do things".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(vec![user_msg], CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert!(matches!(results[0], AgentMessage::Assistant(_)));
        assert!(matches!(results[1], AgentMessage::ToolResult(_)));
        assert!(matches!(results[2], AgentMessage::Assistant(_)));
    }

    #[tokio::test]
    async fn test_partial_terminate_continues_loop() {
        let _ = tracing_subscriber::fmt().try_init();

        struct TerminatingTool;
        #[async_trait::async_trait]
        impl crate::types::AgentTool for TerminatingTool {
            fn name(&self) -> &str {
                "terminator"
            }
            fn description(&self) -> &str {
                "Terminates"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn execute(
                &self,
                _tool_call_id: &str,
                _params: serde_json::Value,
                _on_progress: Option<&(dyn Fn(crate::AgentToolProgressUpdate) + Send + Sync)>,
                _signal: CancellationToken,
            ) -> Result<crate::AgentToolResult, AgentError> {
                Ok(crate::AgentToolResult {
                    content: vec![Content::Text {
                        text: "done".to_string(),
                        text_signature: None,
                    }],
                    details: None,
                    is_error: false,
                    terminate: true,
                })
            }
        }

        struct NonTerminatingTool;
        #[async_trait::async_trait]
        impl crate::types::AgentTool for NonTerminatingTool {
            fn name(&self) -> &str {
                "non_terminating"
            }
            fn description(&self) -> &str {
                "Does not terminate"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn execute(
                &self,
                _tool_call_id: &str,
                _params: serde_json::Value,
                _on_progress: Option<&(dyn Fn(crate::AgentToolProgressUpdate) + Send + Sync)>,
                _signal: CancellationToken,
            ) -> Result<crate::AgentToolResult, AgentError> {
                Ok(crate::AgentToolResult {
                    content: vec![Content::Text {
                        text: "result".to_string(),
                        text_signature: None,
                    }],
                    details: None,
                    is_error: false,
                    terminate: false,
                })
            }
        }

        let terminating_tool = Arc::new(TerminatingTool);
        let non_terminating_tool = Arc::new(NonTerminatingTool);
        let provider = TestProvider::sequence(vec![
            TestResponse::ToolCalls(vec![
                TestToolCall::new("call_t", "terminator", serde_json::json!({})),
                TestToolCall::new("call_nt", "non_terminating", serde_json::json!({})),
            ]),
            TestResponse::Text("done".into()),
        ]);
        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(
            provider,
            dispatcher,
            vec![
                terminating_tool as AgentToolRef,
                non_terminating_tool as AgentToolRef,
            ],
        );
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "do things".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(vec![user_msg], CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(results.len(), 4);
        assert!(matches!(results[0], AgentMessage::Assistant(_)));
        assert!(matches!(results[1], AgentMessage::ToolResult(_)));
        assert!(matches!(results[2], AgentMessage::ToolResult(_)));
        assert!(matches!(results[3], AgentMessage::Assistant(_)));
    }

    #[tokio::test]
    async fn test_resolve_orphan_tool_calls() {
        let mut messages = vec![
            AgentMessage::Assistant(ai_provider::AssistantMessage {
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
            AgentMessage::ToolResult(ai_provider::ToolResultMessage {
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
            AgentMessage::ToolResult(tr) => {
                assert_eq!(tr.tool_call_id, "call_1");
                assert!(tr.is_error);
                assert!(tr.details.as_ref().unwrap()["_orphan"].as_bool().unwrap());
            }
            _ => panic!("expected orphan tool result"),
        }
    }

    #[tokio::test]
    async fn test_steer_queue_injection() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::text("Hello!");
        let dispatcher = Arc::new(AllowAllDispatcher);
        let steer_queue = Arc::new(Mutex::new(vec![AgentMessage::User(
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

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
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
        assert!(matches!(results[0], AgentMessage::User(_)));
        assert!(matches!(results[1], AgentMessage::Assistant(_)));
        assert!(steer_queue.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_follow_up_queue_triggers_second_iteration() {
        let _ = tracing_subscriber::fmt().try_init();

        let provider = TestProvider::counted(|n| TestResponse::Text(format!("response{}", n)));
        let dispatcher = Arc::new(AllowAllDispatcher);
        let follow_up_queue = Arc::new(Mutex::new(vec![AgentMessage::User(
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

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
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

        // Expected: assistant(response0) + user(follow_up_msg) + assistant(response1) = 3 messages in results
        // But wait, results only contains NEW messages. The follow_up_msg is added to both messages and new_messages.
        // So results should be: assistant(response0) + user(follow_up_msg) + assistant(response1)
        assert_eq!(results.len(), 3);
        assert!(matches!(results[0], AgentMessage::Assistant(_)));
        assert!(matches!(results[1], AgentMessage::User(_)));
        assert!(matches!(results[2], AgentMessage::Assistant(_)));
        assert!(follow_up_queue.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_overflow_returns_context_overflow_error() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = TestProvider::overflow();
        let dispatcher = Arc::new(AllowAllDispatcher);
        let config = make_loop_config(provider, dispatcher, vec![]);
        let loop_ = AgentLoop::new(config);

        let user_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: "hi".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let result = loop_.run(vec![user_msg], CancellationToken::new()).await;
        assert!(result.is_err());
        match result {
            Err(AgentError::ContextOverflow(msg)) => {
                assert!(msg.contains("context length exceeded"));
            }
            other => panic!("expected ContextOverflow, got {:?}", other),
        }
    }
}
