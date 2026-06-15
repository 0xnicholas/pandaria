use std::borrow::Cow;

use serde_json::Value;

use crate::{ToolError, ToolParameter, ToolResult};

/// 同步工具核心 trait。
///
/// 所有内置工具与用户自定义工具必须实现此 trait。通过 [`ToolKit`](crate::ToolKit)
/// 注册后，Agent 即可发现与调用。
///
/// # Example
/// ```
/// use std::borrow::Cow;
/// use pawbun_toolkit::{Tool, ToolError, ToolParameter, ToolResult};
/// use serde_json::json;
///
/// #[derive(Debug)]
/// struct EchoTool;
///
/// impl Tool for EchoTool {
///     fn name(&self) -> &str { "echo" }
///     fn description(&self) -> &str { "Echo the input back." }
///
///     fn parameters(&self) -> Cow<'static, [ToolParameter]> {
///         Cow::Owned(vec![
///             ToolParameter {
///                 name: "message".into(),
///                 description: "The message to echo".into(),
///                 required: true,
///                 schema: json!({"type": "string"}),
///             }
///         ])
///     }
///
///     fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
///         Ok(ToolResult {
///             success: true,
///             content: input.into(),
///             metadata: None,
///             elapsed_ms: None,
///         })
///     }
/// }
/// ```
pub trait Tool: std::fmt::Debug + Send + Sync {
    /// 工具唯一标识。
    fn name(&self) -> &str;

    /// 工具功能描述（供 Agent 理解）。
    fn description(&self) -> &str;

    /// 输入参数元数据。
    ///
    /// 返回 [`Cow`] 以允许编译期常量切片或运行时动态生成，避免不必要的堆分配。
    fn parameters(&self) -> Cow<'static, [ToolParameter]>;

    /// 同步执行入口。
    ///
    /// `input` 为 Agent 生成的原始字符串（通常是 JSON）。工具内部应自行解析为结构化参数。
    fn execute(&self, input: &str) -> Result<ToolResult, ToolError>;

    /// 以 [`serde_json::Value`] 形式执行。
    ///
    /// 默认将 `Value` 序列化为字符串后调用 [`execute`](Self::execute)。
    /// 若工具内部已使用 serde 反序列化，可直接覆盖此默认实现以避免双重序列化。
    fn execute_value(&self, input: Value) -> Result<ToolResult, ToolError> {
        let raw =
            serde_json::to_string(&input).map_err(|e| ToolError::serialization(e.to_string()))?;
        self.execute(&raw)
    }

    /// 将自身转换为异步工具引用，用于 `ToolKit` 的调度层。
    ///
    /// 若未实现 [`AsyncTool`](crate::AsyncTool)，返回 [`None`]。
    fn as_async(&self) -> Option<&dyn crate::AsyncTool> {
        None
    }
}
