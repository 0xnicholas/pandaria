use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("unsupported task type: {0}")]
    UnsupportedTask(String),
    #[error("task failed: {0}")]
    TaskFailed(String),
    #[error("task timed out after {0:?}")]
    Timeout(std::time::Duration),
    #[error("task cancelled")]
    Cancelled,
    #[error("download failed: {0}")]
    DownloadFailed(String),
    #[error("file too large: {size} bytes, max: {max} bytes")]
    FileTooLarge { size: u64, max: u64 },
}
