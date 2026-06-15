use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    AsyncToolExecutor, BlockingExecutor, Tool, ToolError, ToolExecutor, ToolRegistry, ToolResult,
};

/// 工具注册表的默认实现。
///
/// `ToolKit` 维护一个 `BTreeMap<String, Arc<dyn Tool>>`，提供工具注册、发现
/// 和同步/异步执行能力。它是大多数 Pandaria Agent 的首选工具容器。
///
/// # Example
/// ```
/// use pawbun_toolkit::{ToolKit, ToolRegistry, ToolExecutor, FileReadTool};
///
/// let mut toolkit = ToolKit::new();
/// toolkit.register(Box::new(FileReadTool::default()));
///
/// assert_eq!(toolkit.len(), 1);
/// assert!(toolkit.get("file_read").is_some());
/// ```
#[derive(Debug, Default)]
pub struct ToolKit {
    tools: BTreeMap<String, Arc<dyn Tool>>,
    default_timeout_ms: Option<u64>,
}

impl ToolKit {
    /// 创建一个空的 `ToolKit`。
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建带有默认超时时间的 `ToolKit`。
    ///
    /// 当通过 [`ToolExecutor::execute`] 调用时，若配置了默认超时，
    /// 将自动通过 [`execute_with_timeout`](Self::execute_with_timeout) 执行。
    ///
    /// **注意**：`default_timeout_ms` 仅影响同步执行。
    /// 异步执行（[`AsyncToolExecutor::execute_async`]）的超时应由调用方
    /// 通过运行时（如 `tokio::time::timeout`）在外层包装实现。
    pub fn with_timeout(ms: u64) -> Self {
        Self {
            tools: BTreeMap::new(),
            default_timeout_ms: Some(ms),
        }
    }

    /// 注册一个工具。
    ///
    /// 若同名工具已存在，将被替换。
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), Arc::from(tool));
    }

    /// 返回已注册工具数量。
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// 返回是否未注册任何工具。
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// 返回默认超时时间（毫秒）。
    pub fn default_timeout_ms(&self) -> Option<u64> {
        self.default_timeout_ms
    }

    /// 单调用超时执行（同步）。
    ///
    /// 在主线程执行工具，同时启动超时监控线程。若工具执行时间超过 `timeout_ms`，
    /// 返回 [`ToolError::Timeout`]。
    ///
    /// **注意**：当前实现不强制中断工具执行（工具会在后台继续运行至完成），
    /// 仅向调用方返回超时错误。未来可通过协作式取消机制改进。
    ///
    /// 异步超时请使用 `tokio::time::timeout` 等运行时原语在外层包装
    /// [`AsyncToolExecutor::execute_async`]。
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, input), fields(tool = %name)))]
    pub fn execute_with_timeout(
        &self,
        name: &str,
        input: &str,
        timeout_ms: u64,
    ) -> Result<ToolResult, ToolError> {
        use std::sync::mpsc;
        use std::time::Duration;

        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.into()))?;

        // 启动超时监控线程。当工具执行完成后，drop done_tx 会立即唤醒超时线程，
        // 避免原来的 sleep(timeout_ms) 导致线程长期存活的问题。
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let timeout_handle = std::thread::spawn(move || {
            match done_rx.recv_timeout(Duration::from_millis(timeout_ms)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => false,
                Err(mpsc::RecvTimeoutError::Timeout) => true,
            }
        });

        let start = std::time::Instant::now();
        let result = tool.execute(input);
        let elapsed = start.elapsed().as_millis() as u64;

        // 通知超时线程：工具已完成（无论成功或失败）
        drop(done_tx);
        let timed_out = timeout_handle
            .join()
            .map_err(|_| ToolError::execution_failed("timeout monitor thread panicked"))?;

        if timed_out {
            return Err(ToolError::Timeout(timeout_ms));
        }

        match result {
            Ok(mut r) => {
                r.elapsed_ms = Some(elapsed);
                Ok(r)
            }
            Err(e) => Err(e),
        }
    }
}

impl ToolRegistry for ToolKit {
    fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    fn list(&self) -> Vec<&dyn Tool> {
        self.tools.values().map(|t| t.as_ref()).collect()
    }

    fn descriptions(&self) -> String {
        let mut parts = Vec::with_capacity(self.tools.len());
        for tool in self.tools.values() {
            let params: Vec<String> = tool
                .parameters()
                .iter()
                .map(|p| {
                    format!(
                        "  - {}: {} (required: {})",
                        p.name, p.description, p.required
                    )
                })
                .collect();

            parts.push(format!(
                "{}: {}\nparameters:\n{}",
                tool.name(),
                tool.description(),
                params.join("\n")
            ));
        }
        parts.join("\n\n")
    }
}

impl ToolExecutor for ToolKit {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, input), fields(tool = %name)))]
    fn execute(&self, name: &str, input: &str) -> Result<ToolResult, ToolError> {
        match self.default_timeout_ms {
            Some(ms) => self.execute_with_timeout(name, input, ms),
            None => {
                let tool = self
                    .tools
                    .get(name)
                    .ok_or_else(|| ToolError::NotFound(name.into()))?;

                let start = std::time::Instant::now();
                let mut result = tool.execute(input)?;
                result.elapsed_ms = Some(start.elapsed().as_millis() as u64);

                #[cfg(feature = "tracing")]
                tracing::debug!(tool = %name, elapsed_ms = result.elapsed_ms.unwrap_or(0), "tool executed");

                Ok(result)
            }
        }
    }
}

#[async_trait]
impl AsyncToolExecutor for ToolKit {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, input, executor), fields(tool = %name)))]
    async fn execute_async(
        &self,
        name: &str,
        input: &str,
        executor: &dyn BlockingExecutor,
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.into()))?;

        // 1. 若工具实现了 AsyncTool，直接调用 execute_async
        if let Some(async_tool) = tool.as_async() {
            let input = input.to_string();
            let start = std::time::Instant::now();
            let mut result = async_tool.execute_async(&input).await?;
            result.elapsed_ms = Some(start.elapsed().as_millis() as u64);

            #[cfg(feature = "tracing")]
            tracing::debug!(tool = %name, elapsed_ms = result.elapsed_ms.unwrap_or(0), "async tool executed");

            return Ok(result);
        }

        // 2. 否则，通过 BlockingExecutor 将同步 execute 投递到阻塞线程池
        let tool = Arc::clone(tool);
        let input = input.to_string();
        executor
            .spawn_blocking(Box::new(move || {
                let start = std::time::Instant::now();
                let mut result = tool.execute(&input)?;
                result.elapsed_ms = Some(start.elapsed().as_millis() as u64);
                Ok(result)
            }))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use serde_json::json;

    use super::*;
    use crate::{Tool, ToolParameter};

    #[derive(Debug)]
    struct DummyTool;

    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "dummy"
        }

        fn description(&self) -> &str {
            "A dummy tool for testing."
        }

        fn parameters(&self) -> Cow<'static, [ToolParameter]> {
            Cow::Owned(vec![ToolParameter {
                name: "input".into(),
                description: "Any string".into(),
                required: true,
                schema: json!({"type": "string"}),
            }])
        }

        fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                success: true,
                content: format!("dummy: {}", input),
                metadata: None,
                elapsed_ms: None,
            })
        }
    }

    #[test]
    fn test_register_and_get() {
        let mut kit = ToolKit::new();
        kit.register(Box::new(DummyTool));

        assert_eq!(kit.len(), 1);
        assert!(kit.get("dummy").is_some());
        assert!(kit.get("missing").is_none());
    }

    #[test]
    fn test_list() {
        let mut kit = ToolKit::new();
        kit.register(Box::new(DummyTool));

        let tools = kit.list();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "dummy");
    }

    #[test]
    fn test_execute_success() {
        let mut kit = ToolKit::new();
        kit.register(Box::new(DummyTool));

        let result = kit.execute("dummy", "hello").unwrap();
        assert!(result.success);
        assert_eq!(result.content, "dummy: hello");
        assert!(result.elapsed_ms.is_some());
    }

    #[test]
    fn test_execute_not_found() {
        let kit = ToolKit::new();
        let err = kit.execute("missing", "input").unwrap_err();
        assert_eq!(err.to_string(), "tool not found: missing");
    }

    #[test]
    fn test_descriptions() {
        let mut kit = ToolKit::new();
        kit.register(Box::new(DummyTool));

        let desc = kit.descriptions();
        assert!(desc.contains("dummy"));
        assert!(desc.contains("A dummy tool for testing."));
        assert!(desc.contains("input"));
    }

    #[test]
    fn test_with_timeout() {
        let kit = ToolKit::with_timeout(5000);
        assert_eq!(kit.default_timeout_ms(), Some(5000));
    }

    #[test]
    fn test_replace_existing() {
        let mut kit = ToolKit::new();
        kit.register(Box::new(DummyTool));
        kit.register(Box::new(DummyTool));
        assert_eq!(kit.len(), 1);
    }

    #[test]
    fn test_execute_with_timeout_success() {
        let mut kit = ToolKit::new();
        kit.register(Box::new(DummyTool));

        let result = kit.execute_with_timeout("dummy", "hello", 5000).unwrap();
        assert!(result.success);
        assert_eq!(result.content, "dummy: hello");
        assert!(result.elapsed_ms.is_some());
    }

    #[test]
    fn test_execute_with_timeout_expired() {
        use std::thread;
        use std::time::Duration;

        #[derive(Debug)]
        struct SlowTool;

        impl Tool for SlowTool {
            fn name(&self) -> &str {
                "slow"
            }
            fn description(&self) -> &str {
                "A slow tool."
            }
            fn parameters(&self) -> Cow<'static, [ToolParameter]> {
                Cow::Owned(vec![])
            }
            fn execute(&self, _input: &str) -> Result<ToolResult, ToolError> {
                thread::sleep(Duration::from_millis(200));
                Ok(ToolResult {
                    success: true,
                    content: "done".into(),
                    metadata: None,
                    elapsed_ms: None,
                })
            }
        }

        let mut kit = ToolKit::new();
        kit.register(Box::new(SlowTool));

        let err = kit.execute_with_timeout("slow", "", 50).unwrap_err();
        assert!(err.to_string().contains("timeout after 50ms"));
    }

    #[test]
    fn test_default_timeout_auto_applied() {
        use std::thread;
        use std::time::Duration;

        #[derive(Debug)]
        struct SlowTool;

        impl Tool for SlowTool {
            fn name(&self) -> &str {
                "slow"
            }
            fn description(&self) -> &str {
                "A slow tool."
            }
            fn parameters(&self) -> Cow<'static, [ToolParameter]> {
                Cow::Owned(vec![])
            }
            fn execute(&self, _input: &str) -> Result<ToolResult, ToolError> {
                thread::sleep(Duration::from_millis(200));
                Ok(ToolResult {
                    success: true,
                    content: "done".into(),
                    metadata: None,
                    elapsed_ms: None,
                })
            }
        }

        let mut kit = ToolKit::with_timeout(50);
        kit.register(Box::new(SlowTool));

        // 通过 ToolExecutor::execute 调用，应自动应用 default_timeout_ms
        let err = kit.execute("slow", "").unwrap_err();
        assert!(err.to_string().contains("timeout after 50ms"));
    }

    #[tokio::test]
    async fn test_execute_async_sync_tool() {
        let mut kit = ToolKit::new();
        kit.register(Box::new(DummyTool));

        let executor = TestExecutor;
        let result = kit
            .execute_async("dummy", "hello", &executor)
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.content, "dummy: hello");
    }

    /// 测试用的 BlockingExecutor 实现（基于 tokio spawn_blocking）。
    pub struct TestExecutor;

    impl BlockingExecutor for TestExecutor {
        fn spawn_blocking(
            &self,
            f: Box<dyn FnOnce() -> Result<ToolResult, ToolError> + Send>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<ToolResult, ToolError>> + Send>,
        > {
            Box::pin(async move {
                tokio::task::spawn_blocking(f).await.unwrap_or_else(|_| {
                    Err(ToolError::execution_failed("blocking task panicked"))
                })
            })
        }
    }
}
