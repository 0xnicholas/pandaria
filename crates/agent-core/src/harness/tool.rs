use std::sync::Arc;

use ai_provider::ToolCall;
use tracing::{info, warn};

use crate::hook::context::{ToolCallCtx, ToolResultCtx};
use crate::error::AgentError;
use crate::hook::dispatcher::HookDispatcher;
use crate::hook::mutations::HookDecision;
use crate::types::{AgentToolProgressUpdate, AgentToolRef};
use crate::hook::timeout::with_timeout;
use crate::utils::helpers::catch_panic;
use ai_provider::ToolResultMessage as ToolResultMsg;

/// Executes a tool call through the full pipeline:
/// prepare → on_tool_call (blocking) → execute → on_tool_result (chain) → finalize.
pub(crate) struct ToolExecutor {
    tenant_id: String,
    session_id: String,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    tool: AgentToolRef,
}

impl ToolExecutor {
    pub(crate) fn new(
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
    pub(crate) async fn execute_tool_call(
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
        let (decision, mutation) = with_timeout(
            self.hook_dispatcher.on_tool_call(&tool_call_ctx),
            500,
            (HookDecision::Continue, crate::mutations::ToolCallMutation::default()),
            "on_tool_call",
        ).await;

        // Apply accumulated input mutation from hook chain
        let tool_input = mutation.input.unwrap_or_else(|| tool_call.arguments.clone());

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

        // Step 2: Execute the tool (with panic boundary per ADR constraint)
        let mut result = catch_panic(
            self.tool.execute(&tool_call.id, tool_input, on_progress)
        ).await??;

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
        let mutation = with_timeout(
            self.hook_dispatcher.on_tool_result(&tool_result_ctx),
            500,
            crate::mutations::ToolResultMutation::default(),
            "on_tool_result",
        ).await;

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
    use crate::hook::mutations::ToolResultMutation;
    use crate::AgentToolResult;
    use ai_provider::Content;

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
        async fn on_tool_call(
            &self,
            _ctx: &ToolCallCtx,
        ) -> (HookDecision, crate::mutations::ToolCallMutation) {
            (
                HookDecision::Block { reason: "test block".to_string() },
                crate::mutations::ToolCallMutation::default(),
            )
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

    // ============================================================================
    // Additional tool.rs tests
    // ============================================================================

    struct ProgressTool;
    #[async_trait]
    impl crate::types::AgentTool for ProgressTool {
        fn name(&self) -> &str { "progress_tool" }
        fn description(&self) -> &str { "Reports progress" }
        fn parameters(&self) -> serde_json::Value { serde_json::json!({}) }
        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
        ) -> Result<AgentToolResult, AgentError> {
            if let Some(callback) = on_progress {
                callback(AgentToolProgressUpdate {
                    content: "step 1".to_string(),
                });
                callback(AgentToolProgressUpdate {
                    content: "step 2".to_string(),
                });
            }
            Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: "done".to_string(),
                    text_signature: None,
                }],
                details: None,
                is_error: false,
                terminate: false,
            })
        }
    }

    #[tokio::test]
    async fn test_tool_progress_callback() {
        let dispatcher = Arc::new(AllowAllDispatcher);
        let tool = Arc::new(ProgressTool);
        let executor = ToolExecutor::new(
            "t1".to_string(),
            "s1".to_string(),
            dispatcher,
            tool,
        );

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "progress_tool".to_string(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        };

        let progress_updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let progress_updates_clone = progress_updates.clone();

        let result = executor
            .execute_tool_call(&tool_call, Some(&move |update: AgentToolProgressUpdate| {
                progress_updates_clone.lock().unwrap().push(update.content);
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        let updates = progress_updates.lock().unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0], "step 1");
        assert_eq!(updates[1], "step 2");
    }

    struct MutatingDispatcher;
    #[async_trait]
    impl HookDispatcher for MutatingDispatcher {
        async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
            ToolResultMutation {
                content: Some(vec![Content::Text {
                    text: "mutated result".to_string(),
                    text_signature: None,
                }]),
                details: Some(serde_json::json!({"mutated": true})),
                is_error: Some(true),
                terminate: None,
            }
        }
    }

    #[tokio::test]
    async fn test_tool_result_mutation() {
        let dispatcher = Arc::new(MutatingDispatcher);
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
        assert!(result.is_error); // mutated to error
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            Content::Text { text, .. } => assert_eq!(text, "mutated result"),
            _ => panic!("expected text content"),
        }
        let details = result.details.unwrap();
        assert_eq!(details["mutated"], true);
    }

    struct ErrorTool;
    #[async_trait]
    impl crate::types::AgentTool for ErrorTool {
        fn name(&self) -> &str { "error_tool" }
        fn description(&self) -> &str { "Always fails" }
        fn parameters(&self) -> serde_json::Value { serde_json::json!({}) }
        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
        ) -> Result<AgentToolResult, AgentError> {
            Err(AgentError::ToolExecutionFailed("intentional failure".to_string()))
        }
    }

    #[tokio::test]
    async fn test_tool_execution_error() {
        let dispatcher = Arc::new(AllowAllDispatcher);
        let tool = Arc::new(ErrorTool);
        let executor = ToolExecutor::new(
            "t1".to_string(),
            "s1".to_string(),
            dispatcher,
            tool,
        );

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "error_tool".to_string(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        };

        let result = executor.execute_tool_call(&tool_call, None).await;
        assert!(result.is_err());
        match result {
            Err(AgentError::ToolExecutionFailed(msg)) => {
                assert_eq!(msg, "intentional failure");
            }
            other => panic!("expected ToolExecutionFailed, got {:?}", other),
        }
    }

    struct TerminateTool;
    #[async_trait]
    impl crate::types::AgentTool for TerminateTool {
        fn name(&self) -> &str { "terminate_tool" }
        fn description(&self) -> &str { "Signals termination" }
        fn parameters(&self) -> serde_json::Value { serde_json::json!({}) }
        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
        ) -> Result<AgentToolResult, AgentError> {
            Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: "terminating".to_string(),
                    text_signature: None,
                }],
                details: Some(serde_json::json!({"special": true})),
                is_error: false,
                terminate: true,
            })
        }
    }

    #[tokio::test]
    async fn test_tool_terminate_flag_propagation() {
        let dispatcher = Arc::new(AllowAllDispatcher);
        let tool = Arc::new(TerminateTool);
        let executor = ToolExecutor::new(
            "t1".to_string(),
            "s1".to_string(),
            dispatcher,
            tool,
        );

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "terminate_tool".to_string(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        };

        let result = executor.execute_tool_call(&tool_call, None).await.unwrap();
        assert!(!result.is_error);
        let details = result.details.unwrap();
        // Verify terminate flag is embedded in details
        assert_eq!(details["_terminate"], true);
        assert_eq!(details["special"], true);
    }

    struct PanicOnToolCallDispatcher;
    #[async_trait]
    impl HookDispatcher for PanicOnToolCallDispatcher {
        async fn on_tool_call(
            &self,
            _ctx: &ToolCallCtx,
        ) -> (HookDecision, crate::mutations::ToolCallMutation) {
            panic!("on_tool_call panic");
        }
    }

    #[tokio::test]
    async fn test_on_tool_call_panic_uses_default() {
        let dispatcher = Arc::new(PanicOnToolCallDispatcher);
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

        // Panic in on_tool_call is caught and defaults to Continue, so tool executes normally
        let result = executor.execute_tool_call(&tool_call, None).await;
        assert!(result.is_ok());
        let content = result.unwrap().content;
        assert_eq!(content.len(), 1);
        assert!(matches!(&content[0], Content::Text { text, .. } if text == "result"));
    }

    struct PanicOnToolResultDispatcher;
    #[async_trait]
    impl HookDispatcher for PanicOnToolResultDispatcher {
        async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
            panic!("on_tool_result panic");
        }
    }

    #[tokio::test]
    async fn test_on_tool_result_panic_uses_default() {
        let dispatcher = Arc::new(PanicOnToolResultDispatcher);
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

        // Panic in on_tool_result is caught and defaults to no mutation
        let result = executor.execute_tool_call(&tool_call, None).await;
        assert!(result.is_ok());
        let content = result.unwrap().content;
        assert_eq!(content.len(), 1);
        assert!(matches!(
            &content[0],
            Content::Text { text, .. } if text == "result"
        ));
    }

    struct PanicTool;
    #[async_trait]
    impl crate::types::AgentTool for PanicTool {
        fn name(&self) -> &str { "panic_tool" }
        fn description(&self) -> &str { "Always panics" }
        fn parameters(&self) -> serde_json::Value { serde_json::json!({}) }
        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
        ) -> Result<AgentToolResult, AgentError> {
            panic!("tool execution panic");
        }
    }

    #[tokio::test]
    async fn test_tool_execute_panic_is_caught() {
        let dispatcher = Arc::new(AllowAllDispatcher);
        let tool = Arc::new(PanicTool);
        let executor = ToolExecutor::new(
            "t1".to_string(),
            "s1".to_string(),
            dispatcher,
            tool,
        );

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "panic_tool".to_string(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        };

        let result = executor.execute_tool_call(&tool_call, None).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AgentError::ToolExecutionFailed(_)));
    }

    struct TerminateMutatingDispatcher;
    #[async_trait]
    impl HookDispatcher for TerminateMutatingDispatcher {
        async fn on_tool_result(
            &self,
            _ctx: &ToolResultCtx,
        ) -> ToolResultMutation {
            ToolResultMutation {
                terminate: Some(true),
                ..Default::default()
            }
        }
    }

    #[tokio::test]
    async fn test_tool_result_mutation_terminate_flag() {
        let dispatcher = Arc::new(TerminateMutatingDispatcher);
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
        let details = result.details.unwrap();
        // Verify terminate flag from mutation is embedded in details
        assert_eq!(details["_terminate"], true);
    }
}
