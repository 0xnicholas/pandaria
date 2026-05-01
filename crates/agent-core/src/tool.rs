use std::sync::Arc;

use llm_client::ToolCall;

use crate::error::AgentError;
use crate::hook_dispatcher::HookDispatcher;
use crate::mutations::HookDecision;
use crate::types::AgentToolRef;
use llm_client::ToolResultMessage as ToolResultMsg;
use crate::context::ToolResultCtx;

pub struct ToolExecutor {
    hook_dispatcher: Arc<dyn HookDispatcher>,
    tool: AgentToolRef,
}

impl ToolExecutor {
    pub fn new(hook_dispatcher: Arc<dyn HookDispatcher>, tool: AgentToolRef) -> Self {
        Self {
            hook_dispatcher,
            tool,
        }
    }

    /// Execute a tool call through the full pipeline:
    /// prepare → on_tool_call → execute → on_tool_result → finalize
    pub async fn execute_tool_call(
        &self,
        tool_call: &ToolCall,
    ) -> Result<ToolResultMsg, AgentError> {
        // Step 1: Dispatch on_tool_call (blocking hook)
        let tool_call_ctx = crate::context::ToolCallCtx {
            tool_name: tool_call.name.clone(),
            tool_call_id: tool_call.id.clone(),
            input: tool_call.arguments.clone(),
        };
        let decision = self.hook_dispatcher.on_tool_call(&tool_call_ctx).await;
        match decision {
            HookDecision::Block { reason } => {
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

        // Step 2: Execute the tool
        let mut result = self.tool.execute(&tool_call.id, tool_call.arguments.clone()).await?;

        // Step 3: Dispatch on_tool_result (chaining hook)
        let tool_result_ctx = ToolResultCtx {
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

        Ok(ToolResultMsg {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content: result.content,
            details: result.details,
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
        ) -> Result<AgentToolResult, AgentError> {
            Ok(AgentToolResult {
                content: vec![Content::Text { text: "result".to_string() }],
                details: None,
                is_error: false,
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
        let executor = ToolExecutor::new(dispatcher, tool);

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "mock_tool".to_string(),
            arguments: serde_json::json!({}),
        };

        let result = executor.execute_tool_call(&tool_call).await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.tool_call_id, "call_1");
    }

    struct BlockAllDispatcher;
    #[async_trait]
    impl HookDispatcher for BlockAllDispatcher {
        async fn on_tool_call(&self, _ctx: &crate::context::ToolCallCtx) -> HookDecision {
            HookDecision::Block { reason: "test block".to_string() }
        }
    }

    #[tokio::test]
    async fn test_tool_executor_blocked() {
        let dispatcher = Arc::new(BlockAllDispatcher);
        let tool = Arc::new(MockTool);
        let executor = ToolExecutor::new(dispatcher, tool);

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "mock_tool".to_string(),
            arguments: serde_json::json!({}),
        };

        let result = executor.execute_tool_call(&tool_call).await.unwrap();
        assert!(result.is_error);
        let details = result.details.unwrap();
        assert_eq!(details["blocked"], true);
    }
}
