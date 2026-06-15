use async_trait::async_trait;

use crate::{BlockingExecutor, Tool, ToolError, ToolResult};

/// 工具发现与注册能力。
///
/// 外部编排器（Agent / Workflow）通过此 trait 发现可用工具，而无需依赖
/// [`ToolKit`](crate::ToolKit) 的具体类型。
///
/// # Example
/// ```
/// use pawbun_toolkit::{ToolKit, ToolRegistry};
///
/// let toolkit = ToolKit::new();
/// let tools = toolkit.list();
/// println!("Registered {} tools", tools.len());
/// ```
pub trait ToolRegistry: Send + Sync {
    /// 按名称获取工具。
    fn get(&self, name: &str) -> Option<&dyn Tool>;

    /// 列出所有已注册工具。
    fn list(&self) -> Vec<&dyn Tool>;

    /// 生成给 LLM 的 function-calling 风格描述文本。
    ///
    /// 输出格式通常遵循 OpenAI / Anthropic 的 tools 字段规范，包含每个工具的
    /// name、description 和 parameters schema。
    fn descriptions(&self) -> String;
}

/// 单工具同步执行能力。
///
/// 外部编排器通过此 trait 调用工具，同样无需依赖 [`ToolKit`](crate::ToolKit)。
///
/// # Example
/// ```
/// use pawbun_toolkit::{ToolExecutor, ToolKit, FileReadTool};
///
/// let mut toolkit = ToolKit::new();
/// toolkit.register(Box::new(FileReadTool::default()));
///
/// // 通过 ToolExecutor trait 调用
/// let result = toolkit.execute("file_read", r#"{"path": "Cargo.toml"}"#);
/// ```
pub trait ToolExecutor: Send + Sync {
    /// 同步执行指定工具。
    ///
    /// 若工具不存在，返回 [`ToolError::NotFound`]。
    fn execute(&self, name: &str, input: &str) -> Result<ToolResult, ToolError>;
}

/// 单工具异步执行能力。
///
/// 需要 `async-trait` 支持。未实现此 trait 的 executor 可在同步线程池中运行。
///
/// 执行策略：
/// 1. 若目标工具实现了 [`AsyncTool`](crate::AsyncTool)，直接调用 `execute_async`。
/// 2. 否则，通过 [`BlockingExecutor`] 将同步 `execute` 投递到阻塞线程池。
#[async_trait]
pub trait AsyncToolExecutor: ToolExecutor {
    /// 异步执行指定工具。
    ///
    /// `executor` 提供阻塞线程池能力，用于将同步工具包装为异步调用。
    async fn execute_async(
        &self,
        name: &str,
        input: &str,
        executor: &dyn BlockingExecutor,
    ) -> Result<ToolResult, ToolError>;
}
