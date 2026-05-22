pub mod error;
pub mod registry;
pub mod task;

pub use error::MediaError;
pub use registry::{MediaModel, MediaModelRegistry};
pub use task::{
    MediaTaskHandle, MediaTaskStatus, MediaTaskType, media_task_type_from_str, next_poll_interval,
};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub enum MediaRequest {
    ImageGeneration {
        prompt: String,
        size: Option<String>,
        style: Option<String>,
        quality: Option<String>,
        n: Option<u32>,
    },
    VideoGeneration {
        prompt: String,
        duration: Option<u32>,
        resolution: Option<String>,
        aspect_ratio: Option<String>,
        image_refs: Vec<MediaReference>,
    },
    AudioGeneration {
        prompt: String,
        voice: Option<String>,
        format: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct MediaReference {
    pub url: String,
    pub mime_type: String,
}

#[derive(Debug, Clone)]
pub enum MediaResponse {
    Inline { data: String, mime_type: String },
    Reference { url: String, mime_type: String },
}

#[async_trait]
pub trait MediaProvider: Send + Sync {
    fn provider_name(&self) -> &str;
    fn supported_tasks(&self) -> Vec<MediaTaskType>;

    async fn generate(
        &self,
        model: &str,
        request: MediaRequest,
        signal: CancellationToken,
    ) -> Result<MediaResponse, MediaError>;

    async fn download(
        &self,
        url: &str,
        max_size: u64,
        signal: CancellationToken,
    ) -> Result<Vec<u8>, MediaError> {
        let client = self.client();
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|e| MediaError::DownloadFailed(e.to_string()))?
            .error_for_status()
            .map_err(|e| MediaError::DownloadFailed(format!("HTTP error: {}", e)))?;

        let mut downloaded: u64 = 0;
        let mut buffer = Vec::new();

        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| MediaError::DownloadFailed(e.to_string()))?
        {
            if signal.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            downloaded += chunk.len() as u64;
            if downloaded > max_size {
                return Err(MediaError::FileTooLarge {
                    size: downloaded,
                    max: max_size,
                });
            }
            buffer.extend_from_slice(&chunk);
        }

        Ok(buffer)
    }

    fn client(&self) -> &reqwest::Client;
}
