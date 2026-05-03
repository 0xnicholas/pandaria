use std::sync::Arc;

use llm_client::{
    Content, LlmContext, LlmProvider, StopReason, StreamOptions, ToolCall,
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, info_span, instrument, warn, Instrument};

use crate::context::{ContextCtx, TurnEndCtx};
use crate::error::AgentError;
use crate::hook_dispatcher::HookDispatcher;
use crate::tool::ToolExecutor;
use crate::types::{AgentMessage, AgentToolRef, ToolExecutionMode};

/// Drives the agent tool-use loop per ADR-001.
///
/// Each turn sends messages to the LLM, receives an AssistantMessage with
/// optional ToolCalls, executes tools, and feeds results back into the loop.
/// The loop terminates when stop_reason is "stop" or all tools signal terminate.
pub struct AgentLoop {
    tenant_id: String,
    session_id: String,
    model: String,
    provider: Arc<dyn LlmProvider>,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    tools: Vec<AgentToolRef>,
}

impl AgentLoop {
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
        }
    }

    /// Run the agent loop for a single prompt.
    /// Returns all new messages generated during the run.
    #[instrument(
        skip(self, initial_messages, signal),
        fields(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            model = %self.model,
        )
    )]
    pub async fn run(
        &self,
        system_prompt: Option<String>,
        initial_messages: Vec<AgentMessage>,
        signal: CancellationToken,
    ) -> Result<Vec<AgentMessage>, AgentError> {
        let mut messages: Vec<AgentMessage> = initial_messages;
        let mut turn_index: u64 = 0;
        let mut new_messages: Vec<AgentMessage> = Vec::new();

        info!(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            msg_count = messages.len(),
            tool_count = self.tools.len(),
            "agent loop started",
        );

        loop {
            if signal.is_cancelled() {
                warn!(
                    tenant_id = %self.tenant_id,
                    session_id = %self.session_id,
                    "agent loop cancelled",
                );
                return Err(AgentError::Cancelled);
            }

            // Transform context via chain hook
            let ctx_ctx = ContextCtx {
                tenant_id: self.tenant_id.clone(),
                session_id: self.session_id.clone(),
                messages: messages.clone(),
            };
            let ctx_mutation = self.hook_dispatcher.on_context(&ctx_ctx).await;
            let transformed = ctx_mutation.messages.unwrap_or_else(|| messages.clone());

            // Build LLM context
            let llm_tools = if self.tools.is_empty() {
                None
            } else {
                Some(
                    self.tools
                        .iter()
                        .map(|t| llm_client::ToolDef {
                            name: t.name().to_string(),
                            description: t.description().to_string(),
                            parameters: t.parameters(),
                        })
                        .collect(),
                )
            };

            let ctx = LlmContext {
                system_prompt: system_prompt.clone(),
                messages: transformed,
                tools: llm_tools,
            };

            // Stream LLM response
            let stream_span = info_span!(
                "llm_stream",
                tenant_id = %self.tenant_id,
                session_id = %self.session_id,
                turn = turn_index,
            );
            let mut stream = self
                .provider
                .stream(
                    &self.model,
                    ctx,
                    StreamOptions::default(),
                    signal.child_token(),
                )
                .instrument(stream_span)
                .await?;

            // Consume stream → AssistantMessage
            let provider_name = self.provider.provider_name().to_string();
            let mut assistant_content: Vec<Content> = Vec::new();
            let mut api = llm_client::Api {
                provider: provider_name.clone(),
                model: self.model.clone(),
            };
            let mut usage = llm_client::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 0,
            };
            let mut stop_reason = StopReason::Stop;
            let mut error_message: Option<String> = None;

            // Per-content_index accumulators for streaming partials
            let mut text_accum: std::collections::BTreeMap<usize, String> =
                std::collections::BTreeMap::new();

            while let Some(event) = stream.next().await {
                if signal.is_cancelled() {
                    return Err(AgentError::Cancelled);
                }
                match event {
                    llm_client::AssistantMessageEvent::Start { .. } => {}
                    llm_client::AssistantMessageEvent::TextStart { .. } => {}
                    llm_client::AssistantMessageEvent::TextDelta {
                        content_index,
                        delta,
                        ..
                    } => {
                        text_accum
                            .entry(content_index)
                            .or_default()
                            .push_str(&delta);
                    }
                    llm_client::AssistantMessageEvent::TextEnd {
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
                    llm_client::AssistantMessageEvent::ThinkingStart { .. } => {}
                    llm_client::AssistantMessageEvent::ThinkingDelta { .. } => {}
                    llm_client::AssistantMessageEvent::ThinkingEnd { .. } => {}
                    llm_client::AssistantMessageEvent::ToolCallStart { .. } => {}
                    llm_client::AssistantMessageEvent::ToolCallDelta { delta: _, .. } => {}
                    llm_client::AssistantMessageEvent::ToolCallEnd { tool_call, .. } => {
                        assistant_content.push(Content::ToolCall(tool_call));
                    }
                    llm_client::AssistantMessageEvent::Done {
                        reason,
                        message,
                    } => {
                        assistant_content = message.content;
                        api = message.api;
                        usage = message.usage;
                        stop_reason = reason;
                    }
                    llm_client::AssistantMessageEvent::Error { error } => {
                        error!(
                            tenant_id = %self.tenant_id,
                            session_id = %self.session_id,
                            llm_error = ?error,
                            "LLM stream error",
                        );
                        error_message = error.error_message;
                        stop_reason = error.stop_reason;
                        break;
                    }
                }
            }

            info!(
                tenant_id = %self.tenant_id,
                session_id = %self.session_id,
                turn = turn_index,
                input_tokens = usage.input_tokens,
                output_tokens = usage.output_tokens,
                stop_reason = ?stop_reason,
                "turn LLM response received",
            );

            let assistant_msg = AgentMessage::Assistant(llm_client::AssistantMessage {
                content: assistant_content.clone(),
                provider: provider_name,
                model: self.model.clone(),
                api,
                usage,
                stop_reason: stop_reason.clone(),
                response_id: None,
                error_message: error_message.clone(),
                timestamp: std::time::SystemTime::now(),
            });
            new_messages.push(assistant_msg.clone());
            messages.push(assistant_msg);

            // Extract ToolCalls
            let tool_calls: Vec<&ToolCall> = assistant_content
                .iter()
                .filter_map(|c| match c {
                    Content::ToolCall(tc) => Some(tc),
                    _ => None,
                })
                .collect();

            if tool_calls.is_empty() {
                // No tool calls — check for error stop reasons first
                match stop_reason {
                    StopReason::Error | StopReason::Aborted | StopReason::Length => {
                        let err_msg = error_message
                            .unwrap_or_else(|| format!("LLM returned stop reason: {:?}", stop_reason));
                        error!(
                            tenant_id = %self.tenant_id,
                            session_id = %self.session_id,
                            turn = turn_index,
                            stop_reason = ?stop_reason,
                            error = %err_msg,
                            "LLM response error",
                        );
                        let turn_ctx = TurnEndCtx {
                            tenant_id: self.tenant_id.clone(),
                            session_id: self.session_id.clone(),
                            turn_index,
                            messages: messages.clone(),
                        };
                        self.hook_dispatcher.on_turn_end(&turn_ctx).await;
                        return Err(AgentError::LlmResponseError(err_msg));
                    }
                    _ => {}
                }
                info!(
                    tenant_id = %self.tenant_id,
                    session_id = %self.session_id,
                    turn = turn_index,
                    "no tool calls, loop complete",
                );
                // Emit turn_end and stop
                let turn_ctx = TurnEndCtx {
                    tenant_id: self.tenant_id.clone(),
                    session_id: self.session_id.clone(),
                    turn_index,
                    messages: messages.clone(),
                };
                self.hook_dispatcher.on_turn_end(&turn_ctx).await;
                break;
            }

            // Determine execution mode: if any tool requires sequential, run all sequentially
            let use_sequential = tool_calls.iter().any(|tc| {
                self.tools
                    .iter()
                    .find(|t| t.name() == tc.name)
                    .map(|t| t.execution_mode() == ToolExecutionMode::Sequential)
                    .unwrap_or(false)
            });

            let mode_label = if use_sequential { "sequential" } else { "parallel" };
            info!(
                tenant_id = %self.tenant_id,
                session_id = %self.session_id,
                turn = turn_index,
                tool_call_count = tool_calls.len(),
                mode = mode_label,
                "executing tool calls",
            );

            // Helper to execute a single tool call
            let execute_one = |tc: &ToolCall,
                               tool: Option<AgentToolRef>,
                               dispatcher: Arc<dyn HookDispatcher>,
                               tenant_id: &str,
                               session_id: &str|
             -> _ {
                let tenant_id = tenant_id.to_string();
                let session_id = session_id.to_string();
                let tc_clone = tc.clone();
                let turn = turn_index;

                async move {
                    match tool {
                        Some(tool) => {
                            let executor = ToolExecutor::new(
                                tenant_id.clone(),
                                session_id.clone(),
                                dispatcher,
                                tool,
                            );
                            let result = executor.execute_tool_call(&tc_clone, None).await;
                            match &result {
                                Ok(msg) => {
                                    let terminated = msg
                                        .details
                                        .as_ref()
                                        .and_then(|d| d.get("_terminate"))
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false);
                                    info!(
                                        tenant_id = %tenant_id,
                                        session_id = %session_id,
                                        turn = turn,
                                        tool_name = %tc_clone.name,
                                        tool_call_id = %tc_clone.id,
                                        terminated = terminated,
                                        "tool call completed",
                                    );
                                    (tc_clone, result, terminated)
                                }
                                Err(e) => {
                                    error!(
                                        tenant_id = %tenant_id,
                                        session_id = %session_id,
                                        turn = turn,
                                        tool_name = %tc_clone.name,
                                        tool_call_id = %tc_clone.id,
                                        error = %e,
                                        "tool call failed",
                                    );
                                    (tc_clone, result, false)
                                }
                            }
                        }
                        None => {
                            warn!(
                                tenant_id = %tenant_id,
                                session_id = %session_id,
                                tool_name = %tc_clone.name,
                                "tool not found",
                            );
                            let err_msg = llm_client::ToolResultMessage {
                                tool_call_id: tc_clone.id.clone(),
                                tool_name: tc_clone.name.clone(),
                                content: vec![],
                                details: Some(serde_json::json!({"error": "tool not found"})),
                                is_error: true,
                                timestamp: std::time::SystemTime::now(),
                            };
                            (tc_clone, Ok(err_msg), false)
                        }
                    }
                }
            };

            let results: Vec<_> = if use_sequential {
                let mut results = Vec::with_capacity(tool_calls.len());
                for tc in &tool_calls {
                    let tool = self
                        .tools
                        .iter()
                        .find(|t| t.name() == tc.name)
                        .cloned();
                    let result = execute_one(
                        tc,
                        tool,
                        self.hook_dispatcher.clone(),
                        &self.tenant_id,
                        &self.session_id,
                    )
                    .await;
                    results.push(result);
                }
                results
            } else {
                let futures: Vec<_> = tool_calls
                    .iter()
                    .map(|tc| {
                        let tool = self
                            .tools
                            .iter()
                            .find(|t| t.name() == tc.name)
                            .cloned();
                        execute_one(
                            tc,
                            tool,
                            self.hook_dispatcher.clone(),
                            &self.tenant_id,
                            &self.session_id,
                        )
                    })
                    .collect();
                futures::future::join_all(futures).await
            };

            let mut all_terminate = !tool_calls.is_empty();
            for (tc, result, terminated) in results {
                match result {
                    Ok(msg) => {
                        new_messages.push(AgentMessage::ToolResult(msg.clone()));
                        messages.push(AgentMessage::ToolResult(msg));
                    }
                    Err(e) => {
                        error!(
                            tenant_id = %self.tenant_id,
                            session_id = %self.session_id,
                            tool_name = %tc.name,
                            error = %e,
                            "unexpected tool execution error",
                        );
                        return Err(e);
                    }
                }
                if !terminated {
                    all_terminate = false;
                }
            }

            // Emit turn_end
            let turn_ctx = TurnEndCtx {
                tenant_id: self.tenant_id.clone(),
                session_id: self.session_id.clone(),
                turn_index,
                messages: messages.clone(),
            };
            self.hook_dispatcher.on_turn_end(&turn_ctx).await;

            // If all tools signaled terminate, stop the loop
            if all_terminate {
                info!(
                    tenant_id = %self.tenant_id,
                    session_id = %self.session_id,
                    "all tools terminated, loop complete",
                );
                break;
            }

            // If the last assistant had stop_reason != ToolUse, break
            if stop_reason != StopReason::ToolUse {
                if stop_reason == StopReason::Error
                    || stop_reason == StopReason::Aborted
                    || stop_reason == StopReason::Length
                {
                    let err_msg = error_message
                        .unwrap_or_else(|| format!("LLM returned stop reason: {:?}", stop_reason));
                    error!(
                        tenant_id = %self.tenant_id,
                        session_id = %self.session_id,
                        turn = turn_index,
                        stop_reason = ?stop_reason,
                        error = %err_msg,
                        "LLM response error after tool execution",
                    );
                    return Err(AgentError::LlmResponseError(err_msg));
                }
                break;
            }

            turn_index += 1;
        }

        info!(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            total_turns = turn_index + 1,
            new_msg_count = new_messages.len(),
            "agent loop finished",
        );

        Ok(new_messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SingleResponseProvider {
        content: Vec<Content>,
        stop_reason: StopReason,
    }

    #[async_trait::async_trait]
    impl LlmProvider for SingleResponseProvider {
        fn provider_name(&self) -> &str {
            "test"
        }
        fn models(&self) -> Vec<String> {
            vec!["test".to_string()]
        }
        async fn stream(
            &self,
            _model: &str,
            _context: LlmContext,
            _options: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<llm_client::AssistantMessageEventStream, llm_client::LlmError> {
            let (mut stream, tx) = llm_client::AssistantMessageEventStream::new(4);
            let provider = "test".to_string();
            let model = "test".to_string();

            let partial = llm_client::AssistantMessage {
                content: self.content.clone(),
                provider: provider.clone(),
                model: model.clone(),
                api: llm_client::Api {
                    provider: provider.clone(),
                    model: model.clone(),
                },
                usage: llm_client::Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: self.stop_reason.clone(),
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            };

            let events = vec![
                llm_client::AssistantMessageEvent::Start {
                    partial: partial.clone(),
                },
                llm_client::AssistantMessageEvent::Done {
                    reason: self.stop_reason.clone(),
                    message: partial,
                },
            ];

            tokio::spawn(async move {
                for event in events {
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
            });

            Ok(stream)
        }
    }

    struct AllowAllDispatcher;
    #[async_trait::async_trait]
    impl HookDispatcher for AllowAllDispatcher {}

    #[tokio::test]
    async fn test_simple_prompt_response() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = Arc::new(SingleResponseProvider {
            content: vec![Content::Text {
                text: "Hello!".to_string(),
                text_signature: None,
            }],
            stop_reason: StopReason::Stop,
        });
        let dispatcher = Arc::new(AllowAllDispatcher);
        let loop_ = AgentLoop::new(
            "t1".to_string(),
            "s1".to_string(),
            "test".to_string(),
            provider,
            dispatcher,
            vec![],
        );

        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text {
                text: "hi".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(
                Some("You are helpful.".to_string()),
                vec![user_msg],
                CancellationToken::new(),
            )
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

    // Mock tool that counts how many times it was executed
    use std::sync::atomic::{AtomicUsize, Ordering};
    use crate::AgentToolProgressUpdate;
    use crate::AgentToolResult;

    struct CounterTool {
        name: String,
        counter: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl crate::types::AgentTool for CounterTool {
        fn name(&self) -> &str { &self.name }
        fn description(&self) -> &str { "A counter tool" }
        fn parameters(&self) -> serde_json::Value { serde_json::json!({}) }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
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

    struct ToolCallProvider {
        tool_calls: Vec<ToolCall>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for ToolCallProvider {
        fn provider_name(&self) -> &str { "tool-call-test" }
        fn models(&self) -> Vec<String> { vec!["test".to_string()] }
        async fn stream(
            &self,
            _model: &str,
            _context: LlmContext,
            _options: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<llm_client::AssistantMessageEventStream, llm_client::LlmError> {
            let (mut stream, tx) = llm_client::AssistantMessageEventStream::new(8);
            let provider = "tool-call-test".to_string();

            let partial = llm_client::AssistantMessage {
                content: self.tool_calls.iter().map(|tc| Content::ToolCall(tc.clone())).collect(),
                provider: provider.clone(),
                model: "test".to_string(),
                api: llm_client::Api { provider, model: "test".to_string() },
                usage: llm_client::Usage {
                    input_tokens: 0, output_tokens: 0,
                    cache_creation_input_tokens: None, cache_read_input_tokens: None,
                    total_tokens: 0,
                },
                stop_reason: StopReason::ToolUse,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            };

            let events = vec![
                llm_client::AssistantMessageEvent::Start { partial: partial.clone() },
                llm_client::AssistantMessageEvent::Done {
                    reason: StopReason::ToolUse,
                    message: partial,
                },
            ];

            tokio::spawn(async move {
                for event in events {
                    if tx.send(event).await.is_err() { break; }
                }
            });

            Ok(stream)
        }
    }

    #[tokio::test]
    async fn test_parallel_tool_execution() {
        let _ = tracing_subscriber::fmt().try_init();

        let tool_a = Arc::new(CounterTool { name: "tool_a".to_string(), counter: AtomicUsize::new(0) });
        let tool_b = Arc::new(CounterTool { name: "tool_b".to_string(), counter: AtomicUsize::new(0) });

        let provider = Arc::new(ToolCallProvider {
            tool_calls: vec![
                ToolCall {
                    id: "call_a".to_string(),
                    name: "tool_a".to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                },
                ToolCall {
                    id: "call_b".to_string(),
                    name: "tool_b".to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                },
            ],
        });

        let dispatcher = Arc::new(AllowAllDispatcher);
        let loop_ = AgentLoop::new(
            "t1".to_string(),
            "s1".to_string(),
            "test".to_string(),
            provider,
            dispatcher,
            vec![tool_a.clone() as AgentToolRef, tool_b.clone() as AgentToolRef],
        );

        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: "do things".to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(Some("You have tools.".to_string()), vec![user_msg], CancellationToken::new())
            .await
            .unwrap();

        // Expected: assistant message + 2 tool results = 3 messages
        assert_eq!(results.len(), 3);
        assert!(matches!(results[0], AgentMessage::Assistant(_)));
        assert!(matches!(results[1], AgentMessage::ToolResult(_)));
        assert!(matches!(results[2], AgentMessage::ToolResult(_)));

        // Both tools should have been executed
        assert_eq!(tool_a.counter.load(Ordering::SeqCst), 1);
        assert_eq!(tool_b.counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_llm_response_error_propagated() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = Arc::new(SingleResponseProvider {
            content: vec![],
            stop_reason: StopReason::Error,
        });
        let dispatcher = Arc::new(AllowAllDispatcher);
        let loop_ = AgentLoop::new(
            "t1".to_string(),
            "s1".to_string(),
            "test".to_string(),
            provider,
            dispatcher,
            vec![],
        );

        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: "hi".to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        });

        let result = loop_
            .run(Some("You are helpful.".to_string()), vec![user_msg], CancellationToken::new())
            .await;

        assert!(result.is_err());
        match result {
            Err(AgentError::LlmResponseError(_)) => {}
            other => panic!("expected LlmResponseError, got {:?}", other),
        }
    }

    // ============================================================================
    // New tests for comprehensive coverage
    // ============================================================================

    #[tokio::test]
    async fn test_sequential_tool_execution() {
        let _ = tracing_subscriber::fmt().try_init();

        struct SequentialCounterTool {
            name: String,
            counter: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl crate::types::AgentTool for SequentialCounterTool {
            fn name(&self) -> &str { &self.name }
            fn description(&self) -> &str { "A sequential counter tool" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({}) }
            fn execution_mode(&self) -> ToolExecutionMode { ToolExecutionMode::Sequential }

            async fn execute(
                &self,
                _tool_call_id: &str,
                _params: serde_json::Value,
                _on_progress: Option<&(dyn Fn(crate::AgentToolProgressUpdate) + Send + Sync)>,
            ) -> Result<crate::AgentToolResult, AgentError> {
                // Add a small delay to make sequential execution observable
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

        let tool_a = Arc::new(SequentialCounterTool { name: "seq_a".to_string(), counter: AtomicUsize::new(0) });
        let tool_b = Arc::new(SequentialCounterTool { name: "seq_b".to_string(), counter: AtomicUsize::new(0) });

        let provider = Arc::new(ToolCallProvider {
            tool_calls: vec![
                ToolCall {
                    id: "call_a".to_string(),
                    name: "seq_a".to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                },
                ToolCall {
                    id: "call_b".to_string(),
                    name: "seq_b".to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                },
            ],
        });

        let dispatcher = Arc::new(AllowAllDispatcher);
        let loop_ = AgentLoop::new(
            "t1".to_string(),
            "s1".to_string(),
            "test".to_string(),
            provider,
            dispatcher,
            vec![tool_a.clone() as AgentToolRef, tool_b.clone() as AgentToolRef],
        );

        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: "do sequential things".to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(Some("You have tools.".to_string()), vec![user_msg], CancellationToken::new())
            .await
            .unwrap();

        // Expected: assistant message + 2 tool results = 3 messages
        assert_eq!(results.len(), 3);
        assert!(matches!(results[0], AgentMessage::Assistant(_)));
        assert!(matches!(results[1], AgentMessage::ToolResult(_)));
        assert!(matches!(results[2], AgentMessage::ToolResult(_)));

        // Both tools should have been executed
        assert_eq!(tool_a.counter.load(Ordering::SeqCst), 1);
        assert_eq!(tool_b.counter.load(Ordering::SeqCst), 1);
    }

    struct ContextMutatingDispatcher;
    #[async_trait::async_trait]
    impl HookDispatcher for ContextMutatingDispatcher {
        async fn on_context(&self, _ctx: &ContextCtx) -> crate::mutations::ContextMutation {
            crate::mutations::ContextMutation {
                messages: Some(vec![
                    AgentMessage::User(llm_client::UserMessage {
                        content: vec![Content::Text {
                            text: "mutated".to_string(),
                            text_signature: None,
                        }],
                        timestamp: std::time::SystemTime::now(),
                    }),
                ]),
            }
        }
    }

    #[tokio::test]
    async fn test_context_hook_mutation() {
        let _ = tracing_subscriber::fmt().try_init();

        // This provider will verify it receives the mutated message
        struct VerifyingProvider;
        #[async_trait::async_trait]
        impl LlmProvider for VerifyingProvider {
            fn provider_name(&self) -> &str { "verify" }
            fn models(&self) -> Vec<String> { vec!["test".to_string()] }
            async fn stream(
                &self,
                _model: &str,
                context: LlmContext,
                _options: StreamOptions,
                _signal: CancellationToken,
            ) -> Result<llm_client::AssistantMessageEventStream, llm_client::LlmError> {
                // Verify the mutated message was passed
                assert_eq!(context.messages.len(), 1);
                match &context.messages[0] {
                    AgentMessage::User(user) => {
                        let text = user.content.first().and_then(|c| match c {
                            Content::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        });
                        assert_eq!(text, Some("mutated"));
                    }
                    _ => panic!("expected user message"),
                }

                let (stream, tx) = llm_client::AssistantMessageEventStream::new(4);
                let partial = llm_client::AssistantMessage {
                    content: vec![Content::Text { text: "ok".to_string(), text_signature: None }],
                    provider: "verify".to_string(),
                    model: "test".to_string(),
                    api: llm_client::Api { provider: "verify".to_string(), model: "test".to_string() },
                    usage: llm_client::Usage {
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
                    let _ = tx.send(llm_client::AssistantMessageEvent::Start { partial: partial.clone() }).await;
                    let _ = tx.send(llm_client::AssistantMessageEvent::Done { reason: StopReason::Stop, message: partial }).await;
                });

                Ok(stream)
            }
        }

        let provider = Arc::new(VerifyingProvider);
        let dispatcher = Arc::new(ContextMutatingDispatcher);
        let loop_ = AgentLoop::new(
            "t1".to_string(),
            "s1".to_string(),
            "test".to_string(),
            provider,
            dispatcher,
            vec![],
        );

        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: "original".to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(None, vec![user_msg], CancellationToken::new())
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
            fn name(&self) -> &str { "terminator" }
            fn description(&self) -> &str { "Terminates the loop" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({}) }

            async fn execute(
                &self,
                _tool_call_id: &str,
                _params: serde_json::Value,
                _on_progress: Option<&(dyn Fn(crate::AgentToolProgressUpdate) + Send + Sync)>,
            ) -> Result<crate::AgentToolResult, AgentError> {
                Ok(crate::AgentToolResult {
                    content: vec![Content::Text { text: "done".to_string(), text_signature: None }],
                    details: None,
                    is_error: false,
                    terminate: true,
                })
            }
        }

        let tool = Arc::new(TerminatingTool);
        let provider = Arc::new(ToolCallProvider {
            tool_calls: vec![
                ToolCall {
                    id: "call_1".to_string(),
                    name: "terminator".to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                },
            ],
        });

        let dispatcher = Arc::new(AllowAllDispatcher);
        let loop_ = AgentLoop::new(
            "t1".to_string(),
            "s1".to_string(),
            "test".to_string(),
            provider,
            dispatcher,
            vec![tool as AgentToolRef],
        );

        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: "terminate".to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(Some("You have tools.".to_string()), vec![user_msg], CancellationToken::new())
            .await
            .unwrap();

        // Should be: assistant + tool_result only (no second LLM call)
        assert_eq!(results.len(), 2);
        assert!(matches!(results[0], AgentMessage::Assistant(_)));
        assert!(matches!(results[1], AgentMessage::ToolResult(_)));
    }

    #[tokio::test]
    async fn test_cancellation_mid_stream() {
        let _ = tracing_subscriber::fmt().try_init();

        struct SlowProvider;
        #[async_trait::async_trait]
        impl LlmProvider for SlowProvider {
            fn provider_name(&self) -> &str { "slow" }
            fn models(&self) -> Vec<String> { vec!["slow".to_string()] }
            async fn stream(
                &self,
                _model: &str,
                _context: LlmContext,
                _options: StreamOptions,
                signal: CancellationToken,
            ) -> Result<llm_client::AssistantMessageEventStream, llm_client::LlmError> {
                let (stream, _tx) = llm_client::AssistantMessageEventStream::new(4);

                // Wait for cancellation
                tokio::select! {
                    _ = signal.cancelled() => {}
                    _ = tokio::time::sleep(tokio::time::Duration::from_secs(10)) => {}
                }

                Err(llm_client::LlmError::Cancelled)
            }
        }

        let provider = Arc::new(SlowProvider);
        let dispatcher = Arc::new(AllowAllDispatcher);
        let loop_ = AgentLoop::new(
            "t1".to_string(),
            "s1".to_string(),
            "slow".to_string(),
            provider,
            dispatcher,
            vec![],
        );

        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: "hi".to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        });

        let cancel_token = CancellationToken::new();
        let cancel_token_clone = cancel_token.child_token();

        let handle = tokio::spawn(async move {
            loop_
                .run(Some("You are helpful.".to_string()), vec![user_msg], cancel_token_clone)
                .await
        });

        // Cancel after a short delay
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        cancel_token.cancel();

        let result = handle.await.unwrap();
        assert!(result.is_err());
        match result {
            Err(AgentError::Cancelled) | Err(AgentError::LlmError(llm_client::LlmError::Cancelled)) => {}
            other => panic!("expected Cancelled, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_empty_tool_set() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = Arc::new(SingleResponseProvider {
            content: vec![Content::Text {
                text: "Hello without tools".to_string(),
                text_signature: None,
            }],
            stop_reason: StopReason::Stop,
        });
        let dispatcher = Arc::new(AllowAllDispatcher);
        let loop_ = AgentLoop::new(
            "t1".to_string(),
            "s1".to_string(),
            "test".to_string(),
            provider,
            dispatcher,
            vec![], // Empty tool set
        );

        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: "hi".to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        });

        let results = loop_
            .run(
                Some("You are helpful.".to_string()),
                vec![user_msg],
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        match &results[0] {
            AgentMessage::Assistant(msg) => {
                assert_eq!(msg.stop_reason, StopReason::Stop);
                assert!(msg.content.iter().any(|c| matches!(c, Content::Text { .. })));
            }
            _ => panic!("expected assistant message"),
        }
    }
}
