use std::borrow::Cow;

use serde_json::json;

use crate::{Tool, ToolError, ToolParameter, ToolResult};

/// 代码执行工具（接口占位）。
///
/// ⚠️ **安全警告**：执行任意代码具有严重安全风险。此工具仅为接口定义，
/// 不提供任何内置沙箱执行能力。实际使用必须配合外部沙箱环境
///（如 Docker、Firejail、e2b 等）。
///
/// 输入为 JSON 字符串，包含：
/// - `code`（字符串）：待执行的代码
/// - `language`（字符串，可选）：编程语言（如 `python`, `javascript`, `rust`）
/// - `timeout_ms`（整数，可选）：执行超时（毫秒）
///
/// # Example
/// ```
/// use pawbun_toolkit::{Tool, CodeExecuteTool};
///
/// let tool = CodeExecuteTool::default();
/// assert_eq!(tool.name(), "code_execute");
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct CodeExecuteTool;

impl Tool for CodeExecuteTool {
    fn name(&self) -> &str {
        "code_execute"
    }

    fn description(&self) -> &str {
        "Execute code in a sandboxed environment. (Placeholder: requires external sandbox integration.)"
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "code".into(),
                description: "Source code to execute".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
            ToolParameter {
                name: "language".into(),
                description: "Programming language (e.g. python, javascript, rust)".into(),
                required: false,
                schema: json!({"type": "string"}),
            },
            ToolParameter {
                name: "timeout_ms".into(),
                description: "Execution timeout in milliseconds".into(),
                required: false,
                schema: json!({"type": "integer"}),
            },
        ])
    }

    fn execute(&self, _input: &str) -> Result<ToolResult, ToolError> {
        Err(ToolError::execution_failed(
            "CodeExecuteTool is a placeholder. Actual code execution requires an external sandbox \
             (e.g. Docker, Firejail, e2b). Do NOT execute untrusted code without proper isolation.",
        ))
    }
}
