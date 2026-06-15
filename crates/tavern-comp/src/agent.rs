use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// Re-export from tavern-core for convenience
pub use tavern_core::SkillConfig;

/// Agent 运行时错误。
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("tool execution error: {0}")]
    ToolError(String),

    #[error("failed to build HTTP client: {0}")]
    ClientBuild(#[from] reqwest::Error),
}

/// 工具定义，在 Hero → Agent 之间传递。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: Value,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default = "default_tool_timeout")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub config: Option<Value>,
}

fn default_tool_timeout() -> u64 {
    30000
}

/// 内置工具 handler 类型。
pub type NativeToolHandler = Arc<
    dyn Fn(Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, AgentError>> + Send>>
        + Send
        + Sync,
>;

/// 内置工具定义。
pub struct NativeTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub handler: NativeToolHandler,
}

/// Agent 运行时 —— LLM 推理 + 工具执行内核。
///
/// 通过 agent-core 直连 LLM provider（RouterProvider 自动路由到 openai/anthropic/deepseek 等）。
/// 支持注册内置工具和远程工具（HttpProxyTool）。
pub struct AgentRuntime {
    native_tools: RwLock<HashMap<String, NativeTool>>,
    /// LLM provider（None = 测试模式，execute 返回错误）
    provider: Arc<dyn ai_provider::LlmProvider>,
}

impl AgentRuntime {
    /// 创建运行时（直接模式，使用 RouterProvider 自动路由 LLM provider）。
    pub fn new() -> Self {
        Self {
            native_tools: RwLock::new(HashMap::new()),
            provider: Arc::new(ai_provider::RouterProvider::new()),
        }
    }

    /// 创建运行时，使用自定义 provider（测试用）。
    pub fn new_with_provider(provider: Arc<dyn ai_provider::LlmProvider>) -> Self {
        Self {
            native_tools: RwLock::new(HashMap::new()),
            provider,
        }
    }

    /// 注册内置工具。
    pub async fn register_native_tool(&self, tool: NativeTool) {
        self.native_tools.write().await.insert(tool.name.clone(), tool);
    }

    /// 执行 Agent 任务。
    ///
    /// 通过 agent-core 直连 LLM provider。
    /// 内置工具注册为 `AgentTool`，远程工具注册为 `HttpProxyTool`。
    pub async fn execute(
        &self,
        agent_id: &str,
        task: &str,
        system_prompt: &str,
        model: &str,
        tools: &[ToolDef],
    ) -> Result<String, AgentError> {
        self.execute_direct(agent_id, task, system_prompt, model, tools).await
    }

    /// 直接模式：通过 agent-core 执行任务（无 HTTP、纯函数调用）。
    ///
    /// 与 `execute()` 语义相同，但不经过 Pandaria HTTP API。
    /// 使用 `ai_provider::RouterProvider` 自动解析 model spec（如 "openai/gpt-4o"）
    /// 并路由到正确的 LLM provider。
    ///
    /// 内置工具注册为 `AgentTool`，远程工具注册为 `HttpProxyTool`。
    pub async fn execute_direct(
        &self,
        agent_id: &str,
        task: &str,
        system_prompt: &str,
        model: &str,
        tools: &[ToolDef],
    ) -> Result<String, AgentError> {
        use std::sync::Arc;
        use std::time::SystemTime;
        use agent_core::{
            AgentLoop, AgentLoopConfig,
            hook::default_dispatcher::DefaultHookDispatcher,
            prompt::PromptBuilder,
        };
        use ai_provider::{Content, Message, UserMessage};
        use tokio_util::sync::CancellationToken;
        use uuid::Uuid;

        let _ = agent_id;

        // 1. 将 Tavern 工具转换为 agent-core AgentToolRef
        let native = self.native_tools.read().await;
        let agent_tools = self.build_agent_tool_refs(tools, &native);
        drop(native);

        // 2. 构建 system prompt
        let prompt_builder = PromptBuilder::from_base(system_prompt.to_string());

        // 3. 创建 AgentLoop 配置
        let provider = self.provider.clone();
        let mut config = AgentLoopConfig::new(
            "tavern".into(),
            Uuid::new_v4().to_string(),
            model.to_string(),
            provider,
            Arc::new(DefaultHookDispatcher::new()),
            agent_tools,
        );
        config.prompt_builder = prompt_builder;

        // 5. 运行 AgentLoop（内部自动处理 tool call 循环）
        let agent_loop = AgentLoop::new(config);
        let user_msg = Message::User(UserMessage {
            content: vec![Content::Text {
                text: task.to_string(),
                text_signature: None,
            }],
            timestamp: SystemTime::now(),
        });

        let result = agent_loop
            .run(vec![user_msg], CancellationToken::new())
            .await
            .map_err(|e| AgentError::ToolError(e.to_string()))?;

        // 6. 提取最终文本响应
        Ok(Self::extract_final_text(&result))
    }

    /// 从 AgentLoop 返回的消息列表中提取最终文本。
    fn extract_final_text(messages: &[ai_provider::Message]) -> String {
        use ai_provider::{Content, Message};

        // Walk backwards to find the last assistant message with text
        for msg in messages.iter().rev() {
            if let Message::Assistant(assistant) = msg {
                let mut text = String::new();
                for part in &assistant.content {
                    if let Content::Text { text: t, .. } = part {
                        text.push_str(t);
                    }
                }
                if !text.is_empty() {
                    return text;
                }
            }
        }
        // Fallback: serialize last message as debug string
        messages
            .last()
            .map(|m| format!("{:?}", m))
            .unwrap_or_default()
    }

    /// 将 Tavern ToolDef + NativeTool 转换为 agent-core AgentToolRef。
    fn build_agent_tool_refs(
        &self,
        remote_tools: &[ToolDef],
        native_tools: &HashMap<String, NativeTool>,
    ) -> Vec<agent_core::AgentToolRef> {
        use std::sync::Arc;
        use agent_core::tools::{AgentTool, AgentToolRef, AgentToolResult, AgentToolProgressUpdate, ToolExecutionMode};
        use agent_core::tools::http_proxy::{HttpProxyTool, ToolConfig};
        use tokio_util::sync::CancellationToken;

        let mut refs: Vec<AgentToolRef> = Vec::new();

        // 内置工具 → 包装为 AgentTool trait 实现
        for (name, nt) in native_tools {
            let handler = nt.handler.clone();
            let desc = nt.description.clone();
            let params = nt.parameters.clone();
            let n = name.clone();

            struct NativeToolAdapter {
                name: String,
                description: String,
                parameters: serde_json::Value,
                handler: super::NativeToolHandler,
            }

            #[async_trait::async_trait]
            impl AgentTool for NativeToolAdapter {
                fn name(&self) -> &str { &self.name }
                fn description(&self) -> &str { &self.description }
                fn parameters(&self) -> serde_json::Value { self.parameters.clone() }
                fn execution_mode(&self) -> ToolExecutionMode { ToolExecutionMode::Sequential }

                async fn execute(
                    &self,
                    _tool_call_id: &str,
                    params: serde_json::Value,
                    _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
                    _signal: CancellationToken,
                ) -> Result<AgentToolResult, agent_core::error::AgentError> {
                    match (self.handler)(params).await {
                        Ok(v) => Ok(AgentToolResult {
                            content: vec![ai_provider::Content::Text {
                                text: v.to_string(),
                                text_signature: None,
                            }],
                            details: None,
                            is_error: false,
                            terminate: false,
                        }),
                        Err(e) => Ok(AgentToolResult {
                            content: vec![ai_provider::Content::Text {
                                text: e.to_string(),
                                text_signature: None,
                            }],
                            details: None,
                            is_error: true,
                            terminate: false,
                        }),
                    }
                }
            }

            refs.push(Arc::new(NativeToolAdapter {
                name: n,
                description: desc,
                parameters: params,
                handler,
            }));
        }

        // 远程工具 → HttpProxyTool
        let secret = std::env::var("TAVERN_TOOL_SECRET").ok();
        let public_url = std::env::var("TAVERN_PUBLIC_URL").unwrap_or_default();
        let client = reqwest::Client::new();
        for t in remote_tools {
            let endpoint = if t.endpoint.is_empty() {
                format!(
                    "{}/api/tools/{}",
                    public_url.trim_end_matches('/'),
                    t.id
                )
            } else {
                t.endpoint.clone()
            };

            let mut headers = std::collections::HashMap::new();
            if let Some(ref s) = secret {
                headers.insert("Authorization".into(), format!("Bearer {}", s));
            }

            let config = ToolConfig {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
                endpoint,
                timeout_ms: Some(t.timeout_ms),
                headers: if headers.is_empty() { None } else { Some(headers) },
            };

            let proxy = HttpProxyTool::new(
                config,
                "tavern".into(),
                uuid::Uuid::new_v4().to_string(),
                client.clone(),
            );
            refs.push(Arc::new(proxy));
        }

        refs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_direct_agent_loop_integration_poc() {
        use std::sync::Arc;
        use std::time::SystemTime;
        use ai_provider::test_utils::MockProvider;
        use ai_provider::{AssistantMessage, Content, Message, UserMessage};
        use agent_core::{
            AgentLoop, AgentLoopConfig,
            hook::default_dispatcher::DefaultHookDispatcher,
        };
        use tokio_util::sync::CancellationToken;

        // 1. Create mock LLM provider — returns "Hello from Pandaria agent-core!"
        let mock = MockProvider::text("Hello from Pandaria agent-core!");

        // 2. Build AgentLoop config (minimal setup, no tools)
        let config = AgentLoopConfig::new(
            "test-tenant".into(),
            "test-session".into(),
            "mock-model".into(),
            Arc::new(mock),
            Arc::new(DefaultHookDispatcher::new()),
            vec![],
        );

        // 3. Create AgentLoop and run with a user message
        let agent_loop = AgentLoop::new(config);
        let user_msg = Message::User(UserMessage {
            content: vec![Content::Text {
                text: "Hi!".into(),
                text_signature: None,
            }],
            timestamp: SystemTime::now(),
        });
        let result = agent_loop
            .run(vec![user_msg], CancellationToken::new())
            .await;

        // 4. Verify direct in-process call succeeds (no HTTP involved)
        assert!(result.is_ok(), "AgentLoop.run() failed: {:?}", result.err());
        let messages = result.unwrap();
        assert!(!messages.is_empty(), "Expected at least one response message");

        // Find the assistant message with our expected text
        let found = messages.iter().any(|m| {
            if let Message::Assistant(AssistantMessage { content, .. }) = m {
                content.iter().any(|c| {
                    matches!(c, Content::Text { text, .. } if text.contains("Pandaria agent-core"))
                })
            } else {
                false
            }
        });
        assert!(found, "Expected response containing 'Pandaria agent-core'");
    }

    #[tokio::test]
    async fn test_tool_adapter_conversion() {
        use std::sync::Arc;
        use ai_provider::test_utils::MockProvider;

        // Create runtime with mock provider
        let mock = Arc::new(MockProvider::text("ok"));
        let runtime = AgentRuntime::new_with_provider(mock);
        runtime.register_native_tool(super::NativeTool {
            name: "echo".into(),
            description: "Echo input".into(),
            parameters: serde_json::json!({"type": "object", "properties": {"msg": {"type": "string"}}}),
            handler: Arc::new(|args: serde_json::Value| {
                Box::pin(async move {
                    let msg = args.get("msg").and_then(|v| v.as_str()).unwrap_or("no msg");
                    Ok(serde_json::Value::String(format!("Echo: {}", msg)))
                })
            }),
        }).await;

        let remote_tool = crate::ToolDef {
            id: "search".into(),
            name: "search".into(),
            description: "Search web".into(),
            parameters: serde_json::json!({"type": "object"}),
            endpoint: String::new(),
            timeout_ms: 30000,
            config: None,
        };

        let native = runtime.native_tools.read().await;
        let agent_tools = runtime.build_agent_tool_refs(&[remote_tool], &native);
        drop(native);

        assert_eq!(agent_tools.len(), 2, "Expected 2 tools (1 native + 1 remote)");
        assert_eq!(agent_tools[0].name(), "echo");
        assert_eq!(agent_tools[1].name(), "search");
    }
}
