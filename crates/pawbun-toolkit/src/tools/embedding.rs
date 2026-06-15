use std::borrow::Cow;

use serde_json::json;

use crate::{Tool, ToolError, ToolParameter, ToolResult};

/// 文本向量化工具（接口占位）。
///
/// **注意**：此工具仅为接口定义，实际 embedding 能力需要外部 embedding 服务
///（如 OpenAI text-embedding-3、本地 embedding 模型等）。
/// 调用 `execute` 将返回错误，提示需要配置外部 embedding 适配器。
///
/// 输入为 JSON 字符串，包含：
/// - `text`（字符串或字符串数组）：待向量化的文本
/// - `model`（字符串，可选）：模型标识符
///
/// # Example
/// ```
/// use pawbun_toolkit::{Tool, EmbeddingTool};
///
/// let tool = EmbeddingTool::default();
/// assert_eq!(tool.name(), "embedding");
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct EmbeddingTool;

impl Tool for EmbeddingTool {
    fn name(&self) -> &str {
        "embedding"
    }

    fn description(&self) -> &str {
        "Generate vector embeddings for text. (Placeholder: requires external embedding service.)"
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "text".into(),
                description: "Text to embed (string or array of strings)".into(),
                required: true,
                schema: json!({"type": ["string", "array"], "items": {"type": "string"}}),
            },
            ToolParameter {
                name: "model".into(),
                description: "Embedding model identifier".into(),
                required: false,
                schema: json!({"type": "string"}),
            },
        ])
    }

    fn execute(&self, _input: &str) -> Result<ToolResult, ToolError> {
        Err(ToolError::execution_failed(
            "EmbeddingTool is a placeholder. Actual embedding generation requires an external embedding service. \
             Please use a concrete implementation that wraps your embedding provider's API.",
        ))
    }
}
