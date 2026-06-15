//! File loading layer: traits and default implementation.

use std::path::PathBuf;

use bytes::Bytes;

use crate::content::{BinaryContent, ImageContent, MediaContent, TextContent};
use crate::file::{File, FileMetadata, FileSource};
use crate::media::{ImageFormat, MediaType};

/// Error type for file loading operations.
#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum LoadError {
    /// IO error (file system, network, etc.).
    #[error("IO error: {message} (kind: {kind:?})")]
    Io {
        /// Error message.
        message: String,
        /// IO error kind.
        kind: std::io::ErrorKind,
    },

    /// Unsupported file format.
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),

    /// Network error during remote fetch.
    #[error("network error: {0}")]
    Network(String),

    /// Path traversal security violation detected.
    #[error("path traversal detected: {0}")]
    PathTraversal(String),

    /// Media type mismatch between expected and detected.
    #[error("media type mismatch: expected {expected}, got {actual}")]
    TypeMismatch {
        /// Expected media type.
        expected: MediaType,
        /// Detected media type.
        actual: MediaType,
    },

    /// File size exceeds configured limit.
    #[error("size exceeded: {actual} bytes (limit {limit})")]
    SizeExceeded {
        /// Actual file size in bytes.
        actual: u64,
        /// Configured size limit in bytes.
        limit: u64,
    },
}

/// Result of loading a file, containing content, metadata, and possible warnings.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LoadedContent {
    /// Parsed media content (text, image, etc.).
    pub content: MediaContent,
    /// File metadata (name, size, MIME type, modification time).
    pub metadata: FileMetadata,
    /// Non-fatal warnings produced during loading and post-processing.
    pub warnings: Vec<String>,
}

/// Synchronous file loader trait.
///
/// Responsible for reading `FileSource` and parsing it into `MediaContent`.
/// **Pure function design**: does not modify the input `File`; all results are returned via `LoadedContent`.
pub trait FileLoader: std::fmt::Debug + Send + Sync {
    /// Loads a single file, returning `LoadedContent`.
    fn load(&self, file: &File) -> Result<LoadedContent, LoadError>;

    /// Batch loads multiple files, default sequential execution.
    /// Output order strictly matches input order.
    fn load_batch<'a>(
        &self,
        files: &'a [File],
    ) -> Vec<(&'a File, Result<LoadedContent, LoadError>)> {
        files.iter().map(|f| (f, self.load(f))).collect()
    }

    /// Retrieves file metadata only, without reading full content (lightweight operation).
    fn metadata(&self, file: &File) -> Result<FileMetadata, LoadError>;
}

/// Asynchronous file loader extension trait.
///
/// **Design trade-off**: `AsyncFileLoader` inherits `FileLoader`, requiring async implementors
/// to also provide synchronous `load()`. This guarantees API consistency.
///
/// **MSRV**: Requires Rust 1.75+ (native `async fn` in trait).
#[allow(async_fn_in_trait)]
pub trait AsyncFileLoader: FileLoader {
    /// Asynchronously loads a single file.
    async fn load_async(&self, file: &File) -> Result<LoadedContent, LoadError>;

    /// Asynchronously batch loads multiple files.
    async fn load_batch_async<'a>(
        &self,
        files: &'a [File],
    ) -> Vec<(&'a File, Result<LoadedContent, LoadError>)> {
        #[cfg(feature = "tokio")]
        {
            let futures: Vec<_> = files
                .iter()
                .map(|f| async move { (f, self.load_async(f).await) })
                .collect();
            futures::future::join_all(futures).await
        }
        #[cfg(not(feature = "tokio"))]
        {
            let mut results = Vec::with_capacity(files.len());
            for f in files {
                results.push((f, self.load_async(f).await));
            }
            results
        }
    }

    /// Asynchronously retrieves file metadata only.
    async fn metadata_async(&self, file: &File) -> Result<FileMetadata, LoadError>;
}

/// Default file loader implementation covering all built-in file sources.
///
/// Supports an optional `base_dir` sandbox. If unset, uses the current working directory.
///
/// **Async support**: `AsyncFileLoader` methods are always available. When the `tokio`
/// feature is enabled, true async filesystem I/O is used; otherwise the sync fallback
/// runs inside the async wrapper (the method is still `async`, but filesystem calls block).
/// Default file loader supporting local, URL, and bytes sources.
#[derive(Debug, Clone)]
pub struct DefaultFileLoader {
    /// Optional base directory for sandboxing local file access.
    pub base_dir: Option<PathBuf>,
}

impl DefaultFileLoader {
    /// Creates a new file loader without a sandbox.
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    /// Creates a new file loader with a base directory sandbox.
    pub fn with_base_dir<P: Into<PathBuf>>(base_dir: P) -> Self {
        Self {
            base_dir: Some(base_dir.into()),
        }
    }

    fn resolve_local_path(&self, file: &File) -> Result<PathBuf, LoadError> {
        let base = match &self.base_dir {
            Some(p) => p
                .canonicalize()
                .map_err(|e| LoadError::Io { message: format!("invalid base dir: {e}"), kind: e.kind() })?,
            None => std::env::current_dir()
                .map_err(|e| LoadError::Io { message: format!("cannot get current dir: {e}"), kind: e.kind() })?,
        };

        let path = match &file.source {
            FileSource::Local { path } => path,
            _ => return Err(LoadError::Io { message: "expected local file source".into(), kind: std::io::ErrorKind::Other }),
        };

        let target = if path.is_absolute() {
            path.canonicalize()
                .map_err(|e| LoadError::Io { message: format!("invalid path: {e}"), kind: e.kind() })?
        } else {
            let joined = base.join(path);
            joined
                .canonicalize()
                .map_err(|e| LoadError::Io { message: format!("invalid path: {e}"), kind: e.kind() })?
        };

        if !target.starts_with(&base) {
            return Err(LoadError::PathTraversal(target.display().to_string()));
        }

        Ok(target)
    }

    fn detect_from_magic_bytes(bytes: &Bytes) -> Option<MediaType> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(MediaType::Image(ImageFormat::Png));
        }
        if bytes.starts_with(b"\xff\xd8") {
            return Some(MediaType::Image(ImageFormat::Jpeg));
        }
        if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            return Some(MediaType::Image(ImageFormat::Gif));
        }
        if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
            return Some(MediaType::Image(ImageFormat::Webp));
        }
        if bytes.starts_with(b"%PDF") {
            return Some(MediaType::Pdf);
        }
        None
    }

    fn detect_media_type(file: &File, bytes: &Bytes) -> MediaType {
        // 1. Magic bytes take priority (cannot be forged by renaming).
        if let Some(ty) = Self::detect_from_magic_bytes(bytes) {
            return ty;
        }

        // 2. Fall back to file extension hint.
        if let Some(hint) = file.source.hint() {
            if let Some(ext) = std::path::Path::new(&hint)
                .extension()
                .and_then(|e| e.to_str())
            {
                if let Some(ty) = MediaType::from_extension(ext) {
                    return ty;
                }
            }
        }

        // 3. Final fallback: UTF-8 decode for text.
        if std::str::from_utf8(bytes).is_ok() {
            return MediaType::Text;
        }
        MediaType::Binary
    }

    fn build_metadata_from_fs(
        path: &std::path::Path,
        media_type: MediaType,
        metadata: std::fs::Metadata,
    ) -> FileMetadata {
        FileMetadata {
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()),
            mime_type: media_type.mime_type().map(|m| m.to_string()),
            size_bytes: Some(metadata.len()),
            modified_at: metadata.modified().ok(),
        }
    }

    fn build_metadata(
        path: &std::path::Path,
        media_type: MediaType,
    ) -> Result<FileMetadata, LoadError> {
        let metadata = std::fs::metadata(path)
            .map_err(|e| LoadError::Io { message: format!("cannot read metadata: {e}"), kind: e.kind() })?;
        Ok(Self::build_metadata_from_fs(path, media_type, metadata))
    }

    #[cfg(feature = "tokio")]
    async fn async_read(path: &std::path::Path) -> Result<Vec<u8>, LoadError> {
        tokio::fs::read(path)
            .await
            .map_err(|e| LoadError::Io { message: format!("cannot read file: {e}"), kind: e.kind() })
    }

    #[cfg(not(feature = "tokio"))]
    async fn async_read(path: &std::path::Path) -> Result<Vec<u8>, LoadError> {
        std::fs::read(path).map_err(|e| LoadError::Io { message: format!("cannot read file: {e}"), kind: e.kind() })
    }

    #[cfg(feature = "tokio")]
    async fn async_metadata(path: &std::path::Path) -> Result<std::fs::Metadata, LoadError> {
        tokio::fs::metadata(path)
            .await
            .map_err(|e| LoadError::Io { message: format!("cannot read metadata: {e}"), kind: e.kind() })
    }

    #[cfg(not(feature = "tokio"))]
    async fn async_metadata(path: &std::path::Path) -> Result<std::fs::Metadata, LoadError> {
        std::fs::metadata(path).map_err(|e| LoadError::Io { message: format!("cannot read metadata: {e}"), kind: e.kind() })
    }

    fn parse_content(file: &File, bytes: &Bytes) -> Result<MediaContent, LoadError> {
        let detected = Self::detect_media_type(file, bytes);

        // Type mismatch check.
        if let Some(expected) = file.media_type {
            if expected != detected {
                return Err(LoadError::TypeMismatch {
                    expected,
                    actual: detected,
                });
            }
        }

        Ok(match detected {
            MediaType::Text => MediaContent::Text(TextContent {
                text: String::from_utf8_lossy(bytes).into_owned(),
                encoding: Some("utf-8".into()),
            }),
            MediaType::Image(fmt) => {
                #[cfg(feature = "image-meta")]
                let (width, height) = Self::extract_image_dimensions(bytes)
                    .map(|(w, h)| (Some(w), Some(h)))
                    .unwrap_or((None, None));
                #[cfg(not(feature = "image-meta"))]
                let (width, height) = (None, None);
                MediaContent::Image(ImageContent {
                    bytes: bytes.clone(),
                    format: fmt,
                    width,
                    height,
                })
            }
            MediaType::Pdf => MediaContent::Pdf(crate::content::PdfContent {
                bytes: bytes.clone(),
                pages: None,
                text_preview: None,
            }),
            MediaType::Audio(fmt) => MediaContent::Audio(crate::content::AudioContent {
                bytes: bytes.clone(),
                format: fmt,
                duration: None,
                sample_rate: None,
            }),
            MediaType::Video(fmt) => MediaContent::Video(crate::content::VideoContent {
                bytes: bytes.clone(),
                format: fmt,
                duration: None,
                thumbnail: None,
            }),
            MediaType::Binary => MediaContent::Binary(BinaryContent {
                bytes: bytes.clone(),
                guessed_type: Some(detected),
            }),
        })
    }

    fn build_loaded_from_bytes(file: &File, bytes: Bytes) -> Result<LoadedContent, LoadError> {
        let content = Self::parse_content(file, &bytes)?;

        let mut metadata = file.metadata.clone();
        metadata.size_bytes = Some(bytes.len() as u64);
        if metadata.mime_type.is_none() {
            metadata.mime_type = content.media_type().mime_type().map(|m| m.to_string());
        }

        Ok(LoadedContent {
            content,
            metadata,
            warnings: vec![],
        })
    }

    fn load_from_bytes(file: &File) -> Result<LoadedContent, LoadError> {
        let bytes = match &file.source {
            FileSource::Bytes { data } => data.clone(),
            _ => return Err(LoadError::Io { message: "expected bytes source".into(), kind: std::io::ErrorKind::Other }),
        };
        Self::build_loaded_from_bytes(file, bytes)
    }

    /// 默认最大下载大小：100 MiB。
    #[allow(dead_code)]
    const DEFAULT_MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;

    #[cfg(feature = "url-source")]
    fn load_url_sync(url: &str) -> Result<Bytes, LoadError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| LoadError::Network(format!("failed to build HTTP client: {e}")))?;
        let resp = client
            .get(url)
            .send()
            .map_err(|e| LoadError::Network(e.to_string()))?;

        if let Some(len) = resp.content_length() {
            if len > Self::DEFAULT_MAX_DOWNLOAD_BYTES {
                return Err(LoadError::SizeExceeded {
                    actual: len,
                    limit: Self::DEFAULT_MAX_DOWNLOAD_BYTES,
                });
            }
        }

        let bytes = resp
            .bytes()
            .map_err(|e| LoadError::Network(e.to_string()))?;

        if bytes.len() as u64 > Self::DEFAULT_MAX_DOWNLOAD_BYTES {
            return Err(LoadError::SizeExceeded {
                actual: bytes.len() as u64,
                limit: Self::DEFAULT_MAX_DOWNLOAD_BYTES,
            });
        }

        Ok(bytes)
    }

    #[cfg(feature = "url-source")]
    async fn load_url_async(url: &str) -> Result<Bytes, LoadError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| LoadError::Network(format!("failed to build HTTP client: {e}")))?;
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| LoadError::Network(e.to_string()))?;

        if let Some(len) = resp.content_length() {
            if len > Self::DEFAULT_MAX_DOWNLOAD_BYTES {
                return Err(LoadError::SizeExceeded {
                    actual: len,
                    limit: Self::DEFAULT_MAX_DOWNLOAD_BYTES,
                });
            }
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| LoadError::Network(e.to_string()))?;

        if bytes.len() as u64 > Self::DEFAULT_MAX_DOWNLOAD_BYTES {
            return Err(LoadError::SizeExceeded {
                actual: bytes.len() as u64,
                limit: Self::DEFAULT_MAX_DOWNLOAD_BYTES,
            });
        }

        Ok(bytes)
    }

    #[cfg(feature = "tokio")]
    async fn resolve_local_path_async(&self, file: &File) -> Result<PathBuf, LoadError> {
        let base = match &self.base_dir {
            Some(p) => tokio::fs::canonicalize(p)
                .await
                .map_err(|e| LoadError::Io { message: format!("invalid base dir: {e}"), kind: e.kind() })?,
            None => std::env::current_dir()
                .map_err(|e| LoadError::Io { message: format!("cannot get current dir: {e}"), kind: e.kind() })?,
        };

        let path = match &file.source {
            FileSource::Local { path } => path,
            _ => return Err(LoadError::Io { message: "expected local file source".into(), kind: std::io::ErrorKind::Other }),
        };

        let target = if path.is_absolute() {
            tokio::fs::canonicalize(path)
                .await
                .map_err(|e| LoadError::Io { message: format!("invalid path: {e}"), kind: e.kind() })?
        } else {
            let joined = base.join(path);
            tokio::fs::canonicalize(&joined)
                .await
                .map_err(|e| LoadError::Io { message: format!("invalid path: {e}"), kind: e.kind() })?
        };

        if !target.starts_with(&base) {
            return Err(LoadError::PathTraversal(target.display().to_string()));
        }

        Ok(target)
    }

    #[cfg(not(feature = "tokio"))]
    async fn resolve_local_path_async(&self, file: &File) -> Result<PathBuf, LoadError> {
        self.resolve_local_path(file)
    }

    #[cfg(feature = "image-meta")]
    fn extract_image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
        use std::io::Cursor;
        let reader = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        reader.into_dimensions().ok()
    }
}

impl Default for DefaultFileLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl FileLoader for DefaultFileLoader {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(file.source = ?file.source, file.media_type = ?file.media_type)))]
    fn load(&self, file: &File) -> Result<LoadedContent, LoadError> {
        match &file.source {
            FileSource::Local { .. } => {
                let path = self.resolve_local_path(file)?;
                let bytes = std::fs::read(&path)
                    .map_err(|e| LoadError::Io { message: format!("cannot read file: {e}"), kind: e.kind() })?;
                let bytes = Bytes::from(bytes);
                let content = Self::parse_content(file, &bytes)?;
                let metadata = Self::build_metadata(&path, content.media_type())?;
                Ok(LoadedContent {
                    content,
                    metadata,
                    warnings: vec![],
                })
            }
            #[cfg(feature = "url-source")]
            FileSource::Url { url } => {
                let bytes = Self::load_url_sync(url)?;
                Self::build_loaded_from_bytes(file, bytes)
            }
            #[cfg(not(feature = "url-source"))]
            FileSource::Url { .. } => Err(LoadError::UnsupportedFormat(
                "URL source requires url-source feature (Alpha)".into(),
            )),
            FileSource::Bytes { .. } => Self::load_from_bytes(file),
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(file.source = ?file.source)))]
    fn metadata(&self, file: &File) -> Result<FileMetadata, LoadError> {
        match &file.source {
            FileSource::Local { .. } => {
                let path = self.resolve_local_path(file)?;
                let detected = file.media_type.unwrap_or(MediaType::Binary);
                Self::build_metadata(&path, detected)
            }
            FileSource::Url { .. } => Err(LoadError::UnsupportedFormat(
                "URL source requires url-source feature (Alpha)".into(),
            )),
            FileSource::Bytes { data } => {
                let mut meta = file.metadata.clone();
                meta.size_bytes = Some(data.len() as u64);
                Ok(meta)
            }
        }
    }
}

impl AsyncFileLoader for DefaultFileLoader {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(file.source = ?file.source, file.media_type = ?file.media_type)))]
    async fn load_async(&self, file: &File) -> Result<LoadedContent, LoadError> {
        match &file.source {
            FileSource::Local { .. } => {
                let path = self.resolve_local_path_async(file).await?;
                let bytes = Self::async_read(&path).await?;
                let bytes = Bytes::from(bytes);
                let content = Self::parse_content(file, &bytes)?;
                let metadata = Self::async_metadata(&path).await?;
                let media_type = content.media_type();
                Ok(LoadedContent {
                    content,
                    metadata: Self::build_metadata_from_fs(&path, media_type, metadata),
                    warnings: vec![],
                })
            }
            #[cfg(feature = "url-source")]
            FileSource::Url { url } => {
                let bytes = Self::load_url_async(url).await?;
                Self::build_loaded_from_bytes(file, bytes)
            }
            #[cfg(not(feature = "url-source"))]
            FileSource::Url { .. } => Err(LoadError::UnsupportedFormat(
                "URL source requires url-source feature (Alpha)".into(),
            )),
            FileSource::Bytes { .. } => Self::load_from_bytes(file),
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(file.source = ?file.source)))]
    async fn metadata_async(&self, file: &File) -> Result<FileMetadata, LoadError> {
        match &file.source {
            FileSource::Local { .. } => {
                let path = self.resolve_local_path_async(file).await?;
                let detected = file.media_type.unwrap_or(MediaType::Binary);
                let metadata = Self::async_metadata(&path).await?;
                Ok(Self::build_metadata_from_fs(&path, detected, metadata))
            }
            #[cfg(feature = "url-source")]
            FileSource::Url { .. } => {
                // Lightweight: return preset metadata without network round-trip.
                Ok(file.metadata.clone())
            }
            #[cfg(not(feature = "url-source"))]
            FileSource::Url { .. } => Err(LoadError::UnsupportedFormat(
                "URL source requires url-source feature (Alpha)".into(),
            )),
            FileSource::Bytes { data } => {
                let mut meta = file.metadata.clone();
                meta.size_bytes = Some(data.len() as u64);
                Ok(meta)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_load_local_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"hello world").unwrap();
        }

        let loader = DefaultFileLoader::with_base_dir(dir.path());
        let file = File::from_path(&path);
        let loaded = loader.load(&file).unwrap();

        assert_eq!(loaded.content.as_text(), Some("hello world"));
        assert_eq!(loaded.content.media_type(), MediaType::Text);
        assert_eq!(loaded.metadata.size_bytes, Some(11));
        assert_eq!(loaded.metadata.name, Some("hello.txt".into()));
    }

    #[test]
    fn test_load_local_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.png");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            // Minimal PNG header
            f.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        }

        let loader = DefaultFileLoader::with_base_dir(dir.path());
        let file = File::from_path(&path);
        let loaded = loader.load(&file).unwrap();

        assert_eq!(
            loaded.content.media_type(),
            MediaType::Image(ImageFormat::Png)
        );
        assert!(loaded.content.as_bytes().is_some());
    }

    #[test]
    fn test_load_bytes() {
        let data = Bytes::from_static(&[0xFF, 0xFE, 0x00, 0x00]);
        let file = File::from_bytes(data.clone(), "data.bin");
        let loader = DefaultFileLoader::new();
        let loaded = loader.load(&file).unwrap();

        assert_eq!(loaded.content.media_type(), MediaType::Binary);
        assert_eq!(loaded.metadata.size_bytes, Some(4));
    }

    #[test]
    fn test_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        // Create a file in the parent directory of the sandbox.
        let parent_file = dir.path().parent().unwrap().join("pawbun_secret.txt");
        std::fs::write(&parent_file, "secret").unwrap();

        let loader = DefaultFileLoader::with_base_dir(dir.path());
        let file = File::from_path("../pawbun_secret.txt");
        let result = loader.load(&file);
        assert!(
            matches!(result, Err(LoadError::PathTraversal(_))),
            "expected PathTraversal error, got {:?}",
            result
        );

        let _ = std::fs::remove_file(&parent_file);
    }

    #[test]
    fn test_type_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake.png");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"this is text, not png").unwrap();
        }

        let loader = DefaultFileLoader::with_base_dir(dir.path());
        // Force a mismatch by making it non-utf8 binary named as .png with user-specified Text.
        let path2 = dir.path().join("fake2.png");
        {
            let mut f = std::fs::File::create(&path2).unwrap();
            f.write_all(&[0xFF, 0xFE, 0x00, 0x00]).unwrap();
        }
        let file2 = File::from_path(&path2).with_media_type(MediaType::Text);
        let result2 = loader.load(&file2);
        assert!(
            matches!(result2, Err(LoadError::TypeMismatch { .. })),
            "expected TypeMismatch, got {:?}",
            result2
        );
    }

    #[test]
    fn test_metadata_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.txt");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"12345").unwrap();
        }

        let loader = DefaultFileLoader::with_base_dir(dir.path());
        let file = File::from_path(&path);
        let meta = loader.metadata(&file).unwrap();
        assert_eq!(meta.size_bytes, Some(5));
        assert_eq!(meta.name, Some("meta.txt".into()));
    }

    #[test]
    fn test_load_batch() {
        let dir = tempfile::tempdir().unwrap();
        let path1 = dir.path().join("a.txt");
        let path2 = dir.path().join("b.txt");
        std::fs::write(&path1, "alpha").unwrap();
        std::fs::write(&path2, "beta").unwrap();

        let loader = DefaultFileLoader::with_base_dir(dir.path());
        let files = vec![File::from_path(&path1), File::from_path(&path2)];
        let results = loader.load_batch(&files);

        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].1.as_ref().unwrap().content.as_text(),
            Some("alpha")
        );
        assert_eq!(
            results[1].1.as_ref().unwrap().content.as_text(),
            Some("beta")
        );
    }

    // ------------------------------------------------------------------
    // Async tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_load_async_local_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("async.txt");
        std::fs::write(&path, "async hello").unwrap();

        let loader = DefaultFileLoader::with_base_dir(dir.path());
        let file = File::from_path(&path);
        let loaded = loader.load_async(&file).await.unwrap();

        assert_eq!(loaded.content.as_text(), Some("async hello"));
        assert_eq!(loaded.content.media_type(), MediaType::Text);
    }

    #[tokio::test]
    async fn test_load_async_bytes() {
        let data = Bytes::from_static(b"async bytes");
        let file = File::from_bytes(data, "note.txt");
        let loader = DefaultFileLoader::new();
        let loaded = loader.load_async(&file).await.unwrap();

        assert_eq!(loaded.content.as_text(), Some("async bytes"));
    }

    #[tokio::test]
    async fn test_metadata_async_local() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta_async.txt");
        std::fs::write(&path, "12345").unwrap();

        let loader = DefaultFileLoader::with_base_dir(dir.path());
        let file = File::from_path(&path);
        let meta = loader.metadata_async(&file).await.unwrap();
        assert_eq!(meta.size_bytes, Some(5));
    }

    #[tokio::test]
    async fn test_load_batch_async() {
        let dir = tempfile::tempdir().unwrap();
        let path1 = dir.path().join("x.txt");
        let path2 = dir.path().join("y.txt");
        std::fs::write(&path1, "foo").unwrap();
        std::fs::write(&path2, "bar").unwrap();

        let loader = DefaultFileLoader::with_base_dir(dir.path());
        let files = vec![File::from_path(&path1), File::from_path(&path2)];
        let results = loader.load_batch_async(&files).await;

        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].1.as_ref().unwrap().content.as_text(),
            Some("foo")
        );
        assert_eq!(
            results[1].1.as_ref().unwrap().content.as_text(),
            Some("bar")
        );
    }

    #[cfg(feature = "image-meta")]
    #[test]
    fn test_load_local_image_with_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dim.png");
        {
            let img = image::RgbaImage::from_pixel(64, 32, image::Rgba([255, 0, 0, 255]));
            img.save(&path).unwrap();
        }

        let loader = DefaultFileLoader::with_base_dir(dir.path());
        let file = File::from_path(&path);
        let loaded = loader.load(&file).unwrap();

        match loaded.content {
            MediaContent::Image(img) => {
                assert_eq!(img.width, Some(64));
                assert_eq!(img.height, Some(32));
            }
            _ => panic!("expected Image content with dimensions"),
        }
    }
}
