use std::sync::Arc;

use futures::StreamExt;
use llm_client::{
    Content, LlmContext, LlmProvider, StopReason, StreamOptions,
    ToolCall,
};
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::hook_dispatcher::HookDispatcher;
use crate::tool::ToolExecutor;
use crate::types::{AgentMessage, AgentToolRef};

pub struct AgentLoop {
    model: String,
    provider: Arc<dyn LlmProvider>,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    tools: Vec<AgentToolRef>,
}

impl AgentLoop {
    pub fn new(
        model: String,
        provider: Arc<dyn LlmProvider>,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        tools: Vec<AgentToolRef>,
    ) -> Self {
        Self {
            model,
            provider,
            hook_dispatcher,
            tools,
        }
    }

    /// Run the agent loop for a single prompt.
    /// Returns all new messages generated during the run.
    pub async fn run(
        &self,
        system_prompt: Option<String>,
        initial_messages: Vec<AgentMessage>,
        signal: CancellationToken,
    ) -> Result<Vec<AgentMessage>, AgentError> {
        let mut messages: Vec<AgentMessage> = initial_messages;
        let mut turn_index: u64 = 0;
        let mut new_messages: Vec<AgentMessage> = Vec::new();

        loop {
            if signal.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            // Transform context via hook
            let transformed = self.hook_dispatcher.on_context(messages.clone()).await;

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
            let mut stream = self
                .provider
                .stream(
                    &self.model,
                    ctx,
                    StreamOptions::default(),
                    signal.child_token(),
                )
                .await?;

            // Consume stream → AssistantMessage
            let mut assistant_content: Vec<Content> = Vec::new();
            let mut api = llm_client::Api {
                provider: self.provider.provider_name().to_string(),
                model: self.model.clone(),
            };
            let mut usage = llm_client::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            };
            let mut stop_reason = StopReason::Stop;
            let mut error_message: Option<String> = None;

            while let Some(event) = stream.next().await {
                if signal.is_cancelled() {
                    return Err(AgentError::Cancelled);
                }
                match event? {
                    llm_client::AssistantMessageEvent::Start => {}
                    llm_client::AssistantMessageEvent::TextDelta { text } => {
                        assistant_content.push(Content::Text { text });
                    }
                    llm_client::AssistantMessageEvent::ToolCallDelta { tool_call } => {
                        assistant_content.push(Content::ToolCall(tool_call));
                    }
                    llm_client::AssistantMessageEvent::Done {
                        content,
                        api: done_api,
                        usage: done_usage,
                        stop_reason: done_reason,
                    } => {
                        assistant_content = content;
                        api = done_api;
                        usage = done_usage;
                        stop_reason = done_reason;
                    }
                    llm_client::AssistantMessageEvent::Error { message } => {
                        error_message = Some(message);
                        break;
                    }
                }
            }

            let assistant_msg = AgentMessage::Assistant(llm_client::AssistantMessage {
                content: assistant_content.clone(),
                api,
                usage,
                stop_reason: stop_reason.clone(),
                response_id: None,
                error_message,
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
                // No tool calls — emit turn_end and stop
                let turn_ctx = crate::context::TurnEndCtx {
                    turn_index,
                    messages: messages.clone(),
                };
                self.hook_dispatcher.on_turn_end(&turn_ctx).await;
                break;
            }

            // Execute tool calls (sequential for now)
            for tc in &tool_calls {
                let tool = self
                    .tools
                    .iter()
                    .find(|t| t.name() == tc.name)
                    .cloned();

                match tool {
                    Some(tool) => {
                        let executor = ToolExecutor::new(self.hook_dispatcher.clone(), tool);
                        let result = executor.execute_tool_call(tc).await?;
                        new_messages.push(AgentMessage::ToolResult(result.clone()));
                        messages.push(AgentMessage::ToolResult(result));
                    }
                    None => {
                        let err_msg = AgentMessage::ToolResult(llm_client::ToolResultMessage {
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            content: vec![],
                            details: Some(serde_json::json!({"error": "tool not found"})),
                            is_error: true,
                            timestamp: std::time::SystemTime::now(),
                        });
                        new_messages.push(err_msg.clone());
                        messages.push(err_msg);
                    }
                }
            }

            // Emit turn_end
            let turn_ctx = crate::context::TurnEndCtx {
                turn_index,
                messages: messages.clone(),
            };
            self.hook_dispatcher.on_turn_end(&turn_ctx).await;

            // If the last assistant had stop_reason != ToolUse, break
            if stop_reason == StopReason::Stop || stop_reason == StopReason::Error {
                break;
            }

            turn_index += 1;
        }

        Ok(new_messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct SingleResponseProvider {
        content: Vec<Content>,
        stop_reason: StopReason,
    }

    #[async_trait]
    impl LlmProvider for SingleResponseProvider {
        fn provider_name(&self) -> &str { "test" }
        fn models(&self) -> Vec<String> { vec!["test".to_string()] }
        async fn stream(
            &self,
            _model: &str,
            _context: LlmContext,
            _options: StreamOptions,
            _signal: CancellationToken,
        ) -> Result<llm_client::AssistantMessageEventStream, llm_client::LlmError> {
            let content = self.content.clone();
            let stop_reason = self.stop_reason.clone();
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(llm_client::AssistantMessageEvent::Done {
                    content,
                    api: llm_client::Api {
                        provider: "test".to_string(),
                        model: "test".to_string(),
                    },
                    usage: llm_client::Usage {
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
                    },
                    stop_reason,
                }),
            ])))
        }
    }

    struct AllowAllDispatcher;
    #[async_trait]
    impl HookDispatcher for AllowAllDispatcher {}

    #[tokio::test]
    async fn test_simple_prompt_response() {
        let provider = Arc::new(SingleResponseProvider {
            content: vec![Content::Text { text: "Hello!".to_string() }],
            stop_reason: StopReason::Stop,
        });
        let dispatcher = Arc::new(AllowAllDispatcher);
        let loop_ = AgentLoop::new("test".to_string(), provider, dispatcher, vec![]);

        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text: "hi".to_string() }],
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
}
