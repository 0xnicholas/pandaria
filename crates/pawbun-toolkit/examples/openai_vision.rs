//! OpenAI GPT-4o 视觉分析适配器示例。
//!
//! 此示例演示如何为 `VisionTool` 提供一个可运行的 OpenAI API 实现。
//!
//! # 运行前提
//! - 设置环境变量 `OPENAI_API_KEY`
//! - 可选：设置 `OPENAI_API_BASE`（默认 https://api.openai.com/v1）
//!
//! # 运行
//! ```bash
//! cargo run --example openai_vision --features http
//! ```

use async_trait::async_trait;
use base64::Engine;
use pawbun_toolkit::{AsyncTool, AsyncToolExecutor, Tool, ToolError, ToolParameter, ToolResult};
use serde_json::json;
use std::borrow::Cow;

/// OpenAI GPT-4o 视觉分析适配器。
///
/// 输入图片支持两种方式：
/// - Base64 编码的图片数据（`image` 字段以 `data:image/...` 开头）
/// - 图片文件路径（`image` 字段为文件路径，适配器内部读取并转 base64）
#[derive(Debug)]
pub struct OpenAiVisionTool {
    api_key: String,
    api_base: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiVisionTool {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_base: "https://api.openai.com/v1".into(),
            model: "gpt-4o".into(),
            client: reqwest::Client::new(),
        }
    }

    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// 将图片数据或路径转为 OpenAI 要求的 base64 data URL。
    async fn resolve_image(
        &self,
        image: &str,
    ) -> Result<String, ToolError> {
        if image.starts_with("data:image/") {
            return Ok(image.to_string());
        }

        // 从文件路径读取
        let bytes = tokio::fs::read(image)
            .await
            .map_err(|e| ToolError::execution_failed(format!("failed to read image: {e}")).with_source(e))?;

        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let mime = match std::path::Path::new(image)
            .extension()
            .and_then(|e| e.to_str())
        {
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("png") => "image/png",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => "image/png",
        };

        Ok(format!("data:{mime};base64,{b64}"))
    }
}

impl Tool for OpenAiVisionTool {
    fn name(&self) -> &str {
        "vision"
    }

    fn description(&self) -> &str {
        "Analyze an image using OpenAI GPT-4o vision model."
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
            "OpenAiVisionTool requires async execution. Use execute_async instead.",
        ))
    }

    fn as_async(&self) -> Option<&dyn AsyncTool> {
        Some(self)
    }
}

#[async_trait]
impl AsyncTool for OpenAiVisionTool {
    async fn execute_async(
        &self,
        input: &str,
    ) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value = serde_json::from_str(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid JSON: {e}")))?;

        let image = parsed
            .get("image")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'image' field"))?;

        let prompt = parsed
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("Describe this image.");

        let image_url = self.resolve_image(image).await?;

        let body = json!({
            "model": self.model,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": prompt },
                        {
                            "type": "image_url",
                            "image_url": { "url": image_url }
                        }
                    ]
                }
            ],
            "max_tokens": 1024
        });

        let resp = self
            .client
            .post(format!("{}/chat/completions", self.api_base))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ToolError::execution_failed(format!("OpenAI request failed: {e}")).with_source(e))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp.json().await.map_err(|e| {
            ToolError::execution_failed(format!("failed to parse OpenAI response: {e}")).with_source(e)
        })?;

        if !status.is_success() {
            return Err(ToolError::execution_failed(format!(
                "OpenAI API error ({}): {}",
                status,
                resp_body.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error")
            )));
        }

        let content = resp_body
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        Ok(ToolResult {
            success: true,
            content,
            metadata: Some(json!({ "model": self.model })),
            elapsed_ms: None,
        })
    }
}

#[tokio::main]
async fn main() {
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        eprintln!("Please set OPENAI_API_KEY environment variable");
        std::process::exit(1);
    });

    let tool = OpenAiVisionTool::new(api_key);

    let mut toolkit = pawbun_toolkit::ToolKit::new();
    toolkit.register(Box::new(tool));

    // 示例：分析一张图片（请替换为实际路径或 base64 数据）
    let input = json!({
        "image": "path/to/image.png",
        "prompt": "What is in this image?"
    })
    .to_string();

    match toolkit.execute_async("vision", &input, &pawbun_toolkit::TokioExecutor).await {
        Ok(result) => {
            println!("Result: {}", result.content);
        }
        Err(e) => {
            eprintln!("Error: {e}");
        }
    }
}
