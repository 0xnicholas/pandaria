#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaTaskType {
    ImageGeneration,
    VideoGeneration,
    AudioGeneration,
}

/// Asynchronous task handle for providers that require polling (e.g. Seedance).
pub struct MediaTaskHandle {
    pub task_id: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaTaskStatus {
    Queued,
    Running,
    Completed,
    Failed { reason: String },
}

/// Map a string to `MediaTaskType` (used by `MediaGenerationTool` parameter parsing).
pub fn media_task_type_from_str(s: &str) -> Result<MediaTaskType, crate::media::MediaError> {
    match s {
        "image" => Ok(MediaTaskType::ImageGeneration),
        "video" => Ok(MediaTaskType::VideoGeneration),
        "audio" => Ok(MediaTaskType::AudioGeneration),
        _ => Err(crate::media::MediaError::UnsupportedTask(s.to_string())),
    }
}

/// Exponential backoff helper for polling intervals.
pub fn next_poll_interval(
    current: std::time::Duration,
    max: std::time::Duration,
) -> std::time::Duration {
    std::cmp::min(current * 2, max)
}
