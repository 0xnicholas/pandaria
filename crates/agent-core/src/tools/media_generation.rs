use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use ai_provider::media::{
    MediaModelRegistry, MediaProvider, MediaRequest, MediaResponse,
    media_task_type_from_str,
};

use crate::space::AgentSpace;
use crate::types::{AgentTool, AgentToolProgressUpdate, AgentToolResult};
use crate::error::AgentError;

pub struct MediaGenerationTool {
    provider: Arc<dyn MediaProvider>,
    registry: Arc<MediaModelRegistry>,
    space: AgentSpace,
    default_model: String,
    tenant_id: String,
    /// Single file size limit in bytes, default 100MB.
    max_file_size: u64,
}

impl MediaGenerationTool {
    pub fn new(
        provider: Arc<dyn MediaProvider>,
        registry: Arc<MediaModelRegistry>,
        default_model: impl Into<String>,
        tenant_id: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            registry,
            space: AgentSpace::from_env_or_default(),
            default_model: default_model.into(),
            tenant_id: tenant_id.into(),
            max_file_size: 100 * 1024 * 1024,
        }
    }

    pub fn with_space(mut self, space: AgentSpace) -> Self {
        self.space = space;
        self
    }

    pub fn with_max_file_size(mut self, max: u64) -> Self {
        self.max_file_size = max;
        self
    }

    /// Resolve the actual model ID based on media_type and optional explicit model.
    fn resolve_model(&self, media_type: &str, explicit_model: Option<&str>) -> Result<String, AgentError> {
        let task = media_task_type_from_str(media_type)
            .map_err(|e| AgentError::ToolExecutionFailed(e.to_string()))?;
        let candidate = explicit_model.unwrap_or(&self.default_model);
        if let Some(meta) = self.registry.get(candidate) {
            if meta.supported_tasks.contains(&task) {
                return Ok(candidate.to_string());
            }
        }
        // Auto-fallback: find first model in registry that supports this task
        self.registry
            .models_for_provider(self.provider.provider_name())
            .into_iter()
            .find(|m| m.supported_tasks.contains(&task))
            .map(|m| m.id.clone())
            .ok_or_else(|| {
                AgentError::ToolExecutionFailed(format!(
                    "no model supports media_type: {}",
                    media_type
                ))
            })
    }

    async fn save_media_to_workspace(
        &self,
        data: &str,
        mime_type: &str,
    ) -> Result<std::path::PathBuf, AgentError> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| AgentError::ToolExecutionFailed(format!("base64 decode failed: {}", e)))?;
        self.save_media_to_workspace_bytes(&bytes, mime_type).await
    }

    async fn save_media_to_workspace_bytes(
        &self,
        bytes: &[u8],
        mime_type: &str,
    ) -> Result<std::path::PathBuf, AgentError> {
        let workspace = self.space.media_dir(&self.tenant_id);
        tokio::fs::create_dir_all(&workspace)
            .await
            .map_err(|e| AgentError::ToolExecutionFailed(format!("create workspace failed: {}", e)))?;

        if bytes.len() as u64 > self.max_file_size {
            return Err(AgentError::ToolExecutionFailed(format!(
                "file too large: {} bytes, max: {} bytes",
                bytes.len(),
                self.max_file_size
            )));
        }

        let ext = mime_type.split('/').nth(1).unwrap_or("bin");
        let ext = ext.split('+').next().unwrap_or(ext);
        let filename = format!("{}.{}", uuid::Uuid::new_v4(), ext);
        let path = workspace.join(&filename);
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| AgentError::ToolExecutionFailed(format!("write file failed: {}", e)))?;

        Ok(path)
    }
}

#[async_trait]
impl AgentTool for MediaGenerationTool {
    fn name(&self) -> &str {
        "generate_media"
    }

    fn description(&self) -> &str {
        "Generate images, videos, or audio based on a text prompt. \
         Use this when the user asks for visual or audio content. \
         For images under 1MB, returns a base64 inline image; \
         for larger images and all videos/audio, saves to workspace and returns the file path."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "media_type": {
                    "type": "string",
                    "enum": ["image", "video", "audio"],
                    "description": "Type of media to generate"
                },
                "prompt": {
                    "type": "string",
                    "description": "Detailed description of the desired media"
                },
                "model": {
                    "type": "string",
                    "description": "Optional model ID to use. If omitted, uses the default model or auto-selects one that supports the requested media_type."
                },
                "size": {
                    "type": "string",
                    "description": "Size hint. For images: e.g. '1024x1024'. For videos: mapped to 'resolution' field, e.g. '1080p'."
                },
                "duration": {
                    "type": "integer",
                    "description": "Duration in seconds, for video only"
                }
            },
            "required": ["media_type", "prompt"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
        signal: CancellationToken,
    ) -> Result<AgentToolResult, AgentError> {
        let media_type = params["media_type"].as_str()
            .ok_or_else(|| AgentError::ToolExecutionFailed("media_type is required".to_string()))?;
        let prompt = params["prompt"].as_str().unwrap_or("").to_string();
        let explicit_model = params.get("model").and_then(|m| m.as_str());

        if let Some(cb) = on_progress {
            cb(AgentToolProgressUpdate {
                content: format!("正在生成 {}...", media_type),
            });
        }

        let model = self.resolve_model(media_type, explicit_model)?;

        let request = match media_type {
            "image" => MediaRequest::ImageGeneration {
                prompt,
                size: params["size"].as_str().map(|s| s.to_string()),
                style: None,
                quality: None,
                n: Some(1),
            },
            "video" => MediaRequest::VideoGeneration {
                prompt,
                duration: params["duration"].as_u64().map(|d| d as u32),
                resolution: params["size"].as_str().map(|s| s.to_string()),
                aspect_ratio: None,
                image_refs: vec![], // Phase 2: no reference image support yet
            },
            "audio" => MediaRequest::AudioGeneration {
                prompt,
                voice: None,
                format: Some("mp3".to_string()),
            },
            _ => return Err(AgentError::ToolExecutionFailed(format!("unsupported media_type: {}", media_type))),
        };

        let response = self.provider
            .generate(&model, request, signal.clone())
            .await
            .map_err(|e| AgentError::ToolExecutionFailed(e.to_string()))?;

        let (content, mut details) = match response {
            MediaResponse::Inline { data, mime_type } => {
                // data is base64 encoded string; data.len() is base64 string byte length
                // 1MB raw data ≈ 1.33MB base64 string, so we use a simple heuristic
                if mime_type.starts_with("image/") && data.len() < (1024 * 1024) {
                    (
                        vec![ai_provider::Content::Image { data, mime_type }],
                        serde_json::Map::new(),
                    )
                } else {
                    let path = self.save_media_to_workspace(&data, &mime_type).await?;
                    (
                        vec![ai_provider::Content::Text {
                            text: format!("媒体已保存至 {}", path.display()),
                            text_signature: None,
                        }],
                        serde_json::Map::new(),
                    )
                }
            }
            MediaResponse::Reference { url, mime_type } => {
                let bytes = self.provider
                    .download(&url, self.max_file_size, signal)
                    .await
                    .map_err(|e| AgentError::ToolExecutionFailed(e.to_string()))?;
                let path = self.save_media_to_workspace_bytes(&bytes, &mime_type).await?;
                let mut d = serde_json::Map::new();
                d.insert("url".to_string(), serde_json::Value::String(url));
                d.insert("mime_type".to_string(), serde_json::Value::String(mime_type));
                (
                    vec![ai_provider::Content::Text {
                        text: format!("媒体已保存至 {}", path.display()),
                        text_signature: None,
                    }],
                    d,
                )
            }
        };

        // Inject cost info
        if let Some(cost) = self.registry.get(&model).and_then(|m| m.cost_per_call) {
            details.insert("cost_per_call".to_string(), serde_json::json!(cost));
            details.insert("currency".to_string(), serde_json::json!("CNY"));
        }
        details.insert("model".to_string(), serde_json::json!(model));
        details.insert("media_type".to_string(), serde_json::json!(media_type));

        Ok(AgentToolResult {
            content,
            details: Some(serde_json::Value::Object(details)),
            is_error: false,
            terminate: false,
        })
    }
}
