//! OpenAI text-embedding 适配器示例。
//!
//! 此示例演示如何为 `EmbeddingTool` 提供一个可运行的 OpenAI API 实现。
//!
//! # 运行前提
//! - 设置环境变量 `OPENAI_API_KEY`
//! - 可选：设置 `OPENAI_API_BASE`（默认 https://api.openai.com/v1）
//!
//! # 运行
//! ```bash
//! cargo run --example openai_embedding --features http
//! ```

use async_trait::async_trait;
use pawbun_toolkit::{AsyncTool, AsyncToolExecutor, Tool, ToolError, ToolParameter, ToolResult};
use serde_json::json;
use std::borrow::Cow;

/// OpenAI text-embedding-3 适配器。
#[derive(Debug)]
pub struct OpenAiEmbeddingTool {
    api_key: String,
    api_base: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiEmbeddingTool {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_base: "https://api.openai.com/v1".into(),
            model: "text-embedding-3-small".into(),
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
}

impl Tool for OpenAiEmbeddingTool {
    fn name(&self) -> &str {
        "embedding"
    }

    fn description(&self) -> &str {
        "Generate vector embeddings for text using OpenAI."
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
                description: "Embedding model identifier (optional)".into(),
                required: false,
                schema: json!({"type": "string"}),
            },
        ])
    }

    fn execute(&self, _input: &str) -> Result<ToolResult, ToolError> {
        Err(ToolError::execution_failed(
            "OpenAiEmbeddingTool requires async execution. Use execute_async instead.",
        ))
    }

    fn as_async(&self) -> Option<&dyn AsyncTool> {
        Some(self)
    }
}

#[async_trait]
impl AsyncTool for OpenAiEmbeddingTool {
    async fn execute_async(
        &self,
        input: &str,
    ) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value = serde_json::from_str(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid JSON: {e}")))?;

        let input_texts = if let Some(arr) = parsed.get("text").and_then(|v| v.as_array()) {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        } else if let Some(text) = parsed.get("text").and_then(|v| v.as_str()) {
            vec![text.to_string()]
        } else {
            return Err(ToolError::invalid_input("missing or invalid 'text' field"));
        };

        let model = parsed
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.model)
            .to_string();

        let body = json!({
            "input": input_texts,
            "model": model
        });

        let resp = self
            .client
            .post(format!("{}/embeddings", self.api_base))
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

        let embeddings: Vec<Vec<f32>> = resp_body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.get("embedding"))
                    .filter_map(|emb| {
                        emb.as_array().map(|nums| {
                            nums.iter()
                                .filter_map(|n| n.as_f64().map(|f| f as f32))
                                .collect::<Vec<f32>>()
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let content = serde_json::to_string(&embeddings)
            .map_err(|e| ToolError::serialization(e.to_string()).with_source(e))?;

        Ok(ToolResult {
            success: true,
            content,
            metadata: Some(json!({
                "model": model,
                "count": embeddings.len(),
                "dimensions": embeddings.first().map(|v| v.len())
            })),
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

    let tool = OpenAiEmbeddingTool::new(api_key);

    let mut toolkit = pawbun_toolkit::ToolKit::new();
    toolkit.register(Box::new(tool));

    // 示例：嵌入一段文本
    let input = json!({
        "text": "Hello, world!"
    })
    .to_string();

    match toolkit.execute_async("embedding", &input, &pawbun_toolkit::TokioExecutor).await {
        Ok(result) => {
            println!("Result: {}", result.content);
        }
        Err(e) => {
            eprintln!("Error: {e}");
        }
    }
}
