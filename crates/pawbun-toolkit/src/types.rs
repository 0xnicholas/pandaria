use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Describes a tool's input parameter for JSON Schema generation.
///
/// This is the canonical definition of `ToolParameter` used throughout
/// `pawbun-toolkit` and `pawbun-mcp-server`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParameter {
    /// Parameter name.
    pub name: String,
    /// Human-readable description for LLM consumption.
    pub description: String,
    /// Whether this parameter is required.
    pub required: bool,
    /// JSON Schema fragment describing the parameter type.
    pub schema: Value,
}

#[cfg(feature = "schemars")]
impl ToolParameter {
    /// Generates a ToolParameter from a type implementing [`schemars::JsonSchema`].
    ///
    /// Requires the `schemars` feature.
    ///
    /// # Example
    /// ```
    /// use pawbun_toolkit::ToolParameter;
    /// use schemars::JsonSchema;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize, JsonSchema)]
    /// struct MyParams {
    ///     path: String,
    /// }
    ///
    /// let param = ToolParameter::from_schema::<MyParams>("input", "Tool input", true);
    /// assert_eq!(param.name, "input");
    /// ```
    pub fn from_schema<T: schemars::JsonSchema>(
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
    ) -> Self {
        let root = schemars::schema_for!(T);
        let schema =
            serde_json::to_value(root.schema).expect("schema serialization should not fail");
        Self {
            name: name.into(),
            description: description.into(),
            required,
            schema,
        }
    }
}

/// 统一工具执行结果。
///
/// 所有工具无论同步或异步，均返回此结构体。`success` 字段明确区分成功与失败，
/// 便于编排器进行错误处理决策。
///
/// # Example
/// ```
/// use pawbun_toolkit::ToolResult;
///
/// let result = ToolResult {
///     success: true,
///     content: "Hello, world!".into(),
///     metadata: None,
///     elapsed_ms: Some(42),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// 执行是否成功。
    pub success: bool,
    /// 执行返回的文本内容。
    pub content: String,
    /// 附加元数据（如 HTTP 状态码、文件大小等）。
    pub metadata: Option<Value>,
    /// 执行耗时（毫秒），由调用方或拦截层填充。
    pub elapsed_ms: Option<u64>,
}
