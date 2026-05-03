use std::sync::Arc;

use llm_client::ToolCall;
use tracing::{info, warn};

use crate::context::{ToolCallCtx, ToolResultCtx};
use crate::error::AgentError;
use crate::hook_dispatcher::HookDispatcher;
use crate::mutations::HookDecision;
use crate::types::{AgentToolProgressUpdate, AgentToolRef};
use llm_client::ToolResultMessage as ToolResultMsg;

/// Executes a tool call through the full pipeline:
/// prepare → on_tool_call (blocking) → execute → on_tool_result (chain) → finalize.
pub struct ToolExecutor {
    tenant_id: String,
    session_id: String,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    tool: AgentToolRef,
}

impl ToolExecutor {
    pub fn new(
        tenant_id: String,
        session_id: String,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        tool: AgentToolRef,
    ) -> Self {
        Self {
            tenant_id,
            session_id,
            hook_dispatcher,
            tool,
        }
    }

    /// Execute a tool call through the full pipeline.
    ///
    /// `on_progress`: optional callback forwarded to the tool for streaming
    /// progress updates.
    pub async fn execute_tool_call(
        &self,
        tool_call: &ToolCall,
        on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
    ) -> Result<ToolResultMsg, AgentError> {
        // Step 1: Dispatch on_tool_call (blocking hook)
        let tool_call_ctx = ToolCallCtx {
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            tool_name: tool_call.name.clone(),
            tool_call_id: tool_call.id.clone(),
            input: tool_call.arguments.clone(),
        };
        let decision = self.hook_dispatcher.on_tool_call(&tool_call_ctx).await;
        match decision {
            HookDecision::Block { reason } => {
                warn!(
                    tenant_id = %self.tenant_id,
                    session_id = %self.session_id,
                    tool_name = %tool_call.name,
                    reason = %reason,
                    "tool call blocked by hook",
                );
                return Ok(ToolResultMsg {
                    tool_call_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    content: vec![],
                    details: Some(serde_json::json!({"blocked": true, "reason": reason})),
                    is_error: true,
                    timestamp: std::time::SystemTime::now(),
                });
            }
            HookDecision::Continue => {}
        }

        info!(
            tenant_id = %self.tenant_id,
            session_id = %self.session_id,
            tool_name = %tool_call.name,
            tool_call_id = %tool_call.id,
            "executing tool",
        );

        // Step 2: Execute the tool
        let mut result = self.tool.execute(&tool_call.id, tool_call.arguments.clone(), on_progress).await?;

        // Step 3: Dispatch on_tool_result (chaining hook)
        let tool_result_ctx = ToolResultCtx {
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            tool_name: tool_call.name.clone(),
            tool_call_id: tool_call.id.clone(),
            input: tool_call.arguments.clone(),
            content: result.content.clone(),
            details: result.details.clone(),
            is_error: result.is_error,
        };
        let mutation = self.hook_dispatcher.on_tool_result(&tool_result_ctx).await;

        // Apply mutations
        if let Some(content) = mutation.content {
            result.content = content;
        }
        if let Some(details) = mutation.details {
            result.details = Some(details);
        }
        if let Some(is_error) = mutation.is_error {
            result.is_error = is_error;
        }
        if let Some(terminate) = mutation.terminate {
            result.terminate = terminate;
        }

        // Embed terminate flag into details so the agent loop can read it
        let details = {
            let mut d = result.details.unwrap_or(serde_json::json!({}));
            if result.terminate {
                d["_terminate"] = serde_json::json!(true);
            }
            Some(d)
        };

        Ok(ToolResultMsg {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content: result.content,
            details,
            is_error: result.is_error,
            timestamp: std::time::SystemTime::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::AgentToolResult;
    use llm_client::Content;

    struct MockTool;
    #[async_trait]
    impl crate::types::AgentTool for MockTool {
        fn name(&self) -> &str { "mock_tool" }
        fn description(&self) -> &str { "A mock tool" }
        fn parameters(&self) -> serde_json::Value { serde_json::json!({}) }
        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
        ) -> Result<AgentToolResult, AgentError> {
            Ok(AgentToolResult {
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

    struct AllowAllDispatcher;
    #[async_trait]
    impl HookDispatcher for AllowAllDispatcher {}

    #[tokio::test]
    async fn test_tool_executor_normal_flow() {
        let dispatcher = Arc::new(AllowAllDispatcher);
        let tool = Arc::new(MockTool);
        let executor = ToolExecutor::new(
            "t1".to_string(),
            "s1".to_string(),
            dispatcher,
            tool,
        );

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "mock_tool".to_string(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        };

        let result = executor.execute_tool_call(&tool_call, None).await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.tool_call_id, "call_1");
    }

    struct BlockAllDispatcher;
    #[async_trait]
    impl HookDispatcher for BlockAllDispatcher {
        async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> HookDecision {
            HookDecision::Block { reason: "test block".to_string() }
        }
    }

    #[tokio::test]
    async fn test_tool_executor_blocked() {
        let dispatcher = Arc::new(BlockAllDispatcher);
        let tool = Arc::new(MockTool);
        let executor = ToolExecutor::new(
            "t1".to_string(),
            "s1".to_string(),
            dispatcher,
            tool,
        );

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "mock_tool".to_string(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        };

        let result = executor.execute_tool_call(&tool_call, None).await.unwrap();
        assert!(result.is_error);
        let details = result.details.unwrap();
        assert_eq!(details["blocked"], true);
    }
}
