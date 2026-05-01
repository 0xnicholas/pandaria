use std::sync::Arc;

use llm_client::Content;
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::hook_dispatcher::HookDispatcher;
use crate::loop_::AgentLoop;
use crate::types::{AgentMessage, AgentToolRef};
use crate::context::SessionCtx;

pub struct SessionActor {
    model: String,
    system_prompt: String,
    provider: Arc<dyn llm_client::LlmProvider>,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    tools: Vec<AgentToolRef>,
    messages: Vec<AgentMessage>,
    /// Messages queued for injection before the next LLM call
    steer_queue: Vec<AgentMessage>,
    /// Messages queued for injection after the agent would stop
    follow_up_queue: Vec<AgentMessage>,
    abort_token: CancellationToken,
}

impl SessionActor {
    pub fn new(
        system_prompt: String,
        model: String,
        provider: Arc<dyn llm_client::LlmProvider>,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        tools: Vec<AgentToolRef>,
    ) -> Self {
        // Emit session_start
        let session_ctx = SessionCtx {
            system_prompt: system_prompt.clone(),
            tools: tools.iter().map(|t| t.parameters()).collect(),
        };
        let dispatcher = hook_dispatcher.clone();
        tokio::spawn(async move {
            dispatcher.on_session_start(&session_ctx).await;
        });

        Self {
            model,
            system_prompt,
            provider,
            hook_dispatcher,
            tools,
            messages: Vec::new(),
            steer_queue: Vec::new(),
            follow_up_queue: Vec::new(),
            abort_token: CancellationToken::new(),
        }
    }

    /// Send a user message and run the agent loop
    pub async fn prompt(
        &mut self,
        text: String,
    ) -> Result<Vec<AgentMessage>, AgentError> {
        self.abort_token = CancellationToken::new();

        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text }],
            timestamp: std::time::SystemTime::now(),
        });
        self.messages.push(user_msg.clone());

        let loop_ = AgentLoop::new(
            self.model.clone(),
            self.provider.clone(),
            self.hook_dispatcher.clone(),
            self.tools.clone(),
        );

        let new_msgs = loop_
            .run(
                Some(self.system_prompt.clone()),
                self.messages.clone(),
                self.abort_token.child_token(),
            )
            .await?;

        self.messages.extend(new_msgs.clone());
        Ok(new_msgs)
    }

    /// Queue a steering message (injected before next LLM call in current run)
    pub fn steer(&mut self, message: AgentMessage) {
        self.steer_queue.push(message);
    }

    /// Queue a follow-up message (injected after agent would stop)
    pub fn follow_up(&mut self, message: AgentMessage) {
        self.follow_up_queue.push(message);
    }

    /// Abort the current run
    pub fn abort(&self) {
        self.abort_token.cancel();
    }

    /// Get the current message history
    pub fn messages(&self) -> &[AgentMessage] {
        &self.messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

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
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(llm_client::AssistantMessageEvent::Done {
                    content: vec![Content::Text { text: "response".to_string() }],
                    api: llm_client::Api { provider: "echo".to_string(), model: "echo".to_string() },
                    usage: llm_client::Usage { input_tokens: 0, output_tokens: 1, cache_creation_input_tokens: None, cache_read_input_tokens: None },
                    stop_reason: llm_client::StopReason::Stop,
                }),
            ])))
        }
    }

    struct AllowAllDispatcher;
    #[async_trait]
    impl HookDispatcher for AllowAllDispatcher {}

    #[tokio::test]
    async fn test_session_prompt() {
        let provider = Arc::new(EchoProvider);
        let dispatcher = Arc::new(AllowAllDispatcher);
        let mut session = SessionActor::new(
            "You are helpful.".to_string(),
            "echo".to_string(),
            provider,
            dispatcher,
            vec![],
        );

        let results = session.prompt("hello".to_string()).await.unwrap();
        // prompt() returns new messages from the loop (assistant responses + tool results)
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], AgentMessage::Assistant(_)));
        // Session messages include the user message + all loop results
        assert_eq!(session.messages().len(), 2);
    }
}
