use tokio_util::sync::CancellationToken;

use crate::media::{
    MediaError, MediaProvider, MediaRequest, MediaResponse, MediaTaskHandle, MediaTaskStatus,
    MediaTaskType,
};

crate::providers::media_shared::define_media_provider!(
    DoubaoMediaProvider,
    "doubao",
    "DOUBAO_API_KEY",
    "https://ark.cn-beijing.volces.com/api/v3"
);

impl DoubaoMediaProvider {
    async fn generate_image(
        &self,
        model: &str,
        prompt: String,
        size: Option<String>,
    ) -> Result<MediaResponse, MediaError> {
        let url = format!("{}/contents/generations", self.config.base_url);

        let (width, height) = size
            .as_ref()
            .and_then(|s| {
                let mut parts = s.split('x');
                let w = parts.next()?.parse::<u32>().ok()?;
                let h = parts.next()?.parse::<u32>().ok()?;
                Some((w, h))
            })
            .unwrap_or((1024, 1024));

        let body = serde_json::json!({
            "model": model,
            "prompt": prompt,
            "width": width,
            "height": height,
        });

        let response = self
            .config
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| MediaError::TaskFailed(format!("HTTP request failed: {}", e)))?
            .error_for_status()
            .map_err(|e| MediaError::TaskFailed(format!("HTTP error: {}", e)))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| MediaError::TaskFailed(format!("JSON parse failed: {}", e)))?;

        let image_url = response
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("url"))
            .and_then(|u| u.as_str())
            .ok_or_else(|| MediaError::TaskFailed("missing image url in response".to_string()))?;

        Ok(MediaResponse::Reference {
            url: image_url.to_string(),
            mime_type: "image/png".to_string(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn generate_video(
        &self,
        model: &str,
        prompt: String,
        duration: Option<u32>,
        resolution: Option<String>,
        aspect_ratio: Option<String>,
        image_refs: Vec<crate::media::MediaReference>,
        signal: CancellationToken,
    ) -> Result<MediaResponse, MediaError> {
        let task = self
            .create_video_task(
                model,
                &prompt,
                duration,
                resolution.as_deref(),
                aspect_ratio.as_deref(),
                &image_refs,
            )
            .await?;

        let mut interval = std::time::Duration::from_secs(1);
        let max_interval = std::time::Duration::from_secs(30);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300); // 5 min timeout

        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {},
                _ = signal.cancelled() => return Err(MediaError::Cancelled),
            }

            if std::time::Instant::now() > deadline {
                return Err(MediaError::Timeout(std::time::Duration::from_secs(300)));
            }

            let status = self.query_task(&task.task_id).await?;
            match status {
                MediaTaskStatus::Completed => break,
                MediaTaskStatus::Failed { reason } => return Err(MediaError::TaskFailed(reason)),
                _ => interval = std::cmp::min(interval * 2, max_interval),
            }
        }

        self.fetch_video_result(&task.task_id).await
    }

    async fn create_video_task(
        &self,
        model: &str,
        prompt: &str,
        duration: Option<u32>,
        resolution: Option<&str>,
        aspect_ratio: Option<&str>,
        image_refs: &[crate::media::MediaReference],
    ) -> Result<MediaTaskHandle, MediaError> {
        let url = format!("{}/contents/generations/tasks", self.config.base_url);

        let mut body = serde_json::json!({
            "model": model,
            "prompt": prompt,
        });

        if let Some(d) = duration {
            body["duration"] = serde_json::json!(d);
        }
        if let Some(r) = resolution {
            body["resolution"] = serde_json::json!(r);
        }
        if let Some(ar) = aspect_ratio {
            body["aspect_ratio"] = serde_json::json!(ar);
        }
        if !image_refs.is_empty() {
            body["image_refs"] = serde_json::json!(
                image_refs
                    .iter()
                    .map(|r| serde_json::json!({"url": r.url, "mime_type": r.mime_type}))
                    .collect::<Vec<_>>()
            );
        }

        let response = self
            .config
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| MediaError::TaskFailed(format!("HTTP request failed: {}", e)))?
            .error_for_status()
            .map_err(|e| MediaError::TaskFailed(format!("HTTP error: {}", e)))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| MediaError::TaskFailed(format!("JSON parse failed: {}", e)))?;

        let task_id = response
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MediaError::TaskFailed("missing task_id in response".to_string()))?;

        Ok(MediaTaskHandle {
            task_id: task_id.to_string(),
            model: model.to_string(),
        })
    }

    async fn query_task(&self, task_id: &str) -> Result<MediaTaskStatus, MediaError> {
        let url = format!(
            "{}/contents/generations/tasks/{}",
            self.config.base_url, task_id
        );

        let response = self
            .config
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| MediaError::TaskFailed(format!("HTTP request failed: {}", e)))?
            .error_for_status()
            .map_err(|e| MediaError::TaskFailed(format!("HTTP error: {}", e)))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| MediaError::TaskFailed(format!("JSON parse failed: {}", e)))?;

        let status = response
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        match status {
            "queued" => Ok(MediaTaskStatus::Queued),
            "running" => Ok(MediaTaskStatus::Running),
            "completed" => Ok(MediaTaskStatus::Completed),
            "failed" => {
                let reason = response
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                Ok(MediaTaskStatus::Failed { reason })
            }
            _ => Ok(MediaTaskStatus::Failed {
                reason: format!("unknown status: {}", status),
            }),
        }
    }

    async fn fetch_video_result(&self, task_id: &str) -> Result<MediaResponse, MediaError> {
        let url = format!(
            "{}/contents/generations/tasks/{}",
            self.config.base_url, task_id
        );

        let response = self
            .config
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| MediaError::TaskFailed(format!("HTTP request failed: {}", e)))?
            .error_for_status()
            .map_err(|e| MediaError::TaskFailed(format!("HTTP error: {}", e)))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| MediaError::TaskFailed(format!("JSON parse failed: {}", e)))?;

        let video_url = response
            .get("result")
            .and_then(|r| r.get("url"))
            .and_then(|u| u.as_str())
            .ok_or_else(|| MediaError::TaskFailed("missing video url in response".to_string()))?;

        Ok(MediaResponse::Reference {
            url: video_url.to_string(),
            mime_type: "video/mp4".to_string(),
        })
    }
}

#[async_trait::async_trait]
impl MediaProvider for DoubaoMediaProvider {
    fn provider_name(&self) -> &str {
        "doubao"
    }

    fn supported_tasks(&self) -> Vec<MediaTaskType> {
        vec![
            MediaTaskType::ImageGeneration,
            MediaTaskType::VideoGeneration,
        ]
    }

    async fn generate(
        &self,
        model: &str,
        request: MediaRequest,
        signal: CancellationToken,
    ) -> Result<MediaResponse, MediaError> {
        match request {
            MediaRequest::ImageGeneration { prompt, size, .. } => {
                self.generate_image(model, prompt, size).await
            }
            MediaRequest::VideoGeneration {
                prompt,
                duration,
                resolution,
                aspect_ratio,
                image_refs,
            } => {
                self.generate_video(
                    model,
                    prompt,
                    duration,
                    resolution,
                    aspect_ratio,
                    image_refs,
                    signal,
                )
                .await
            }
            _ => Err(MediaError::UnsupportedTask(
                "unsupported media request".to_string(),
            )),
        }
    }

    fn client(&self) -> &reqwest::Client {
        &self.config.client
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[tokio::test]
    async fn test_mock_seedream_image_generation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/contents/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "url": "https://example.com/image.png", "revised_prompt": "revised" }]
            })))
            .mount(&server)
            .await;

        let provider = DoubaoMediaProvider::with_base_url(None, &server.uri());
        let response = provider
            .generate(
                "doubao-seedream-5-0",
                MediaRequest::ImageGeneration {
                    prompt: "a cat".to_string(),
                    size: Some("1024x1024".to_string()),
                    style: None,
                    quality: None,
                    n: Some(1),
                },
                CancellationToken::new(),
            )
            .await;

        assert!(matches!(response, Ok(MediaResponse::Reference { .. })));
        if let Ok(MediaResponse::Reference { url, mime_type }) = response {
            assert_eq!(url, "https://example.com/image.png");
            assert_eq!(mime_type, "image/png");
        }
    }

    #[tokio::test]
    async fn test_mock_seedance_video_generation_async() {
        let server = MockServer::start().await;

        // Mock create task
        Mock::given(method("POST"))
            .and(path("/contents/generations/tasks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "task_id": "task-123"
            })))
            .mount(&server)
            .await;

        // Mock first query: queued
        Mock::given(method("GET"))
            .and(path("/contents/generations/tasks/task-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "queued"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        // Mock second query: running
        Mock::given(method("GET"))
            .and(path("/contents/generations/tasks/task-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "running"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        // Mock third query: completed
        Mock::given(method("GET"))
            .and(path("/contents/generations/tasks/task-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "completed",
                "result": { "url": "https://example.com/video.mp4" }
            })))
            .mount(&server)
            .await;

        let provider = DoubaoMediaProvider::with_base_url(None, &server.uri());
        let response = provider
            .generate(
                "doubao-seedance-2-0",
                MediaRequest::VideoGeneration {
                    prompt: "a dancing cat".to_string(),
                    duration: Some(5),
                    resolution: Some("720p".to_string()),
                    aspect_ratio: None,
                    image_refs: vec![],
                },
                CancellationToken::new(),
            )
            .await;

        assert!(matches!(response, Ok(MediaResponse::Reference { .. })));
        if let Ok(MediaResponse::Reference { url, mime_type }) = response {
            assert_eq!(url, "https://example.com/video.mp4");
            assert_eq!(mime_type, "video/mp4");
        }
    }

    #[tokio::test]
    async fn test_mock_seedance_video_cancelled() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/contents/generations/tasks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "task_id": "task-456"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/contents/generations/tasks/task-456"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "queued"
            })))
            .mount(&server)
            .await;

        let provider = DoubaoMediaProvider::with_base_url(None, &server.uri());
        let signal = CancellationToken::new();
        let signal_clone = signal.clone();

        let handle = tokio::spawn(async move {
            provider
                .generate(
                    "doubao-seedance-2-0",
                    MediaRequest::VideoGeneration {
                        prompt: "a dancing cat".to_string(),
                        duration: None,
                        resolution: None,
                        aspect_ratio: None,
                        image_refs: vec![],
                    },
                    signal_clone,
                )
                .await
        });

        // Cancel immediately
        signal.cancel();
        let result = handle.await.unwrap();
        assert!(matches!(result, Err(MediaError::Cancelled)));
    }
}
