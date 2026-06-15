use std::borrow::Cow;

use serde_json::json;

use crate::{Tool, ToolError, ToolParameter, ToolResult};

/// 图片分析工具（接口占位）。
///
/// **注意**：此工具仅为接口定义，实际视觉分析能力需要外部多模态模型（如 GPT-4V、Claude 3 等）。
/// 调用 `execute` 将返回错误，提示需要配置外部模型适配器。
///
/// 输入为 JSON 字符串，包含：
/// - `image`（字符串）：Base64 编码的图片或图片路径
/// - `prompt`（字符串，可选）：分析提示词，默认 "Describe this image."
///
/// # Example
/// ```
/// use pawbun_toolkit::{Tool, VisionTool};
///
/// let tool = VisionTool::default();
/// assert_eq!(tool.name(), "vision");
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct VisionTool;

impl Tool for VisionTool {
    fn name(&self) -> &str {
        "vision"
    }

    fn description(&self) -> &str {
        "Analyze an image using a vision model. (Placeholder: requires external model integration.)"
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "image".into(),
                description: "Base64-encoded image data or image file path".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
            ToolParameter {
                name: "prompt".into(),
                description: "Prompt for the vision analysis".into(),
                required: false,
                schema: json!({"type": "string"}),
            },
        ])
    }

    fn execute(&self, _input: &str) -> Result<ToolResult, ToolError> {
        Err(ToolError::execution_failed(
            "VisionTool is a placeholder. Actual vision analysis requires an external multimodal model integration. \
             Please use a concrete implementation that wraps your LLM provider's vision API.",
        ))
    }
}
