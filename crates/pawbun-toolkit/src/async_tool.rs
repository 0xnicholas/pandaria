use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use serde_json::Value;

use crate::{Tool, ToolError, ToolResult};

/// 异步工具扩展 trait。
///
/// 需要异步 IO 的工具（如网络请求、MCP 调用）应额外实现此 trait。
/// 未实现此 trait 的工具可通过 [`BlockingExecutor`] 在异步上下文中以阻塞方式运行。
///
/// # Example
/// ```
/// use pawbun_toolkit::{Tool, AsyncTool, ToolError, ToolParameter, ToolResult};
/// use async_trait::async_trait;
/// use serde_json::json;
/// use std::borrow::Cow;
///
/// #[derive(Debug)]
/// struct AsyncEchoTool;
///
/// #[async_trait]
/// impl Tool for AsyncEchoTool {
///     fn name(&self) -> &str { "async_echo" }
///     fn description(&self) -> &str { "Async echo." }
///     fn parameters(&self) -> Cow<'static, [ToolParameter]> {
///         Cow::Owned(vec![])
///     }
///     fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
///         Ok(ToolResult {
///             success: true,
///             content: input.into(),
///             metadata: None,
///             elapsed_ms: None,
///         })
///     }
/// }
///
/// #[async_trait]
/// impl AsyncTool for AsyncEchoTool {
///     async fn execute_async(&self, input: &str) -> Result<ToolResult, ToolError> {
///         Ok(ToolResult {
///             success: true,
///             content: input.into(),
///             metadata: None,
///             elapsed_ms: None,
///         })
///     }
/// }
/// ```
#[async_trait]
pub trait AsyncTool: Tool {
    /// 异步执行入口。
    ///
    /// `input` 为 Agent 生成的原始字符串（通常是 JSON）。
    async fn execute_async(&self, input: &str) -> Result<ToolResult, ToolError>;

    /// 以 [`serde_json::Value`] 形式异步执行。
    ///
    /// 默认将 `Value` 序列化为字符串后调用 [`execute_async`](Self::execute_async)。
    async fn execute_value_async(&self, input: Value) -> Result<ToolResult, ToolError> {
        let raw =
            serde_json::to_string(&input).map_err(|e| ToolError::serialization(e.to_string()))?;
        self.execute_async(&raw).await
    }
}

/// 可插拔的阻塞执行策略，由调用方根据所用运行时提供实现。
///
/// 当异步执行器需要调用仅实现 [`Tool`]（未实现 [`AsyncTool`]）的工具时，
/// 通过此 trait 将同步 `execute` 投递到阻塞线程池，避免阻塞异步事件循环。
///
/// # Example: Tokio 适配器
/// ```
/// use pawbun_toolkit::{BlockingExecutor, ToolError, ToolResult};
/// use std::future::Future;
/// use std::pin::Pin;
///
/// pub struct TokioExecutor;
///
/// impl BlockingExecutor for TokioExecutor {
///     fn spawn_blocking(
///         &self,
///         f: Box<dyn FnOnce() -> Result<ToolResult, ToolError> + Send>,
///     ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send>> {
///         Box::pin(async move {
///             tokio::task::spawn_blocking(move || f())
///                 .await
///                 .unwrap_or_else(|_| Err(ToolError::execution_failed("blocking task panicked")))
///         })
///     }
/// }
/// ```
pub trait BlockingExecutor: Send + Sync {
    /// 在阻塞线程池中执行闭包，返回一个可 await 的 Future。
    fn spawn_blocking(
        &self,
        f: Box<dyn FnOnce() -> Result<ToolResult, ToolError> + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send>>;
}

/// 基于 Tokio 运行时的 [`BlockingExecutor`] 实现。
///
/// 将同步工具的 `execute` 调用通过 [`tokio::task::spawn_blocking`] 投递到 Tokio 的阻塞线程池，
/// 避免在异步上下文中阻塞事件循环。
///
/// 需要启用 `tokio` feature 方可使用。
///
/// # Example
/// ```no_run
/// use pawbun_toolkit::{TokioExecutor, AsyncToolExecutor, ToolKit};
///
/// # async fn example() {
/// let toolkit = ToolKit::new();
/// let executor = TokioExecutor;
/// let result = toolkit.execute_async("some_tool", "{}", &executor).await;
/// # }
/// ```
#[cfg(feature = "tokio")]
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioExecutor;

#[cfg(feature = "tokio")]
impl BlockingExecutor for TokioExecutor {
    fn spawn_blocking(
        &self,
        f: Box<dyn FnOnce() -> Result<ToolResult, ToolError> + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(f).await.unwrap_or_else(|_| {
                Err(ToolError::execution_failed("blocking task panicked"))
            })
        })
    }
}
