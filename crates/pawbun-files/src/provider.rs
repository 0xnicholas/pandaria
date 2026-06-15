//! Provider adapter layer for formatting MediaContent into LLM-specific request structures.

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::content::MediaContent;
use crate::file::File;
use crate::loader::{FileLoader, LoadError};
use crate::media::MediaType;

/// Error type for provider formatting operations.
#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum FormatError {
    /// Provider does not support this media type.
    #[error("unsupported media type for {provider}: {media_type}")]
    UnsupportedMediaType {
        /// Provider name.
        provider: String,
        /// Media type that is not supported.
        media_type: MediaType,
    },
    /// Content size exceeds provider limit.
    #[error("content too large for {provider}: {size} bytes")]
    ContentTooLarge {
        /// Provider name.
        provider: String,
        /// Actual content size in bytes.
        size: u64,
    },
    /// JSON serialization failed.
    #[error("serialization failed: {0}")]
    Serialization(String),
    /// File loading failed before formatting.
    #[error("load failed: {0}")]
    Load(String),
}

impl From<LoadError> for FormatError {
    fn from(err: LoadError) -> Self {
        FormatError::Load(err.to_string())
    }
}

/// Provider constraints for a given media type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConstraints {
    /// Maximum bytes per file. Using `u64` to support 32-bit systems and large files.
    pub max_size_bytes: u64,
    /// Maximum files per request.
    pub max_files_per_request: usize,
    /// Supported MIME type whitelist.
    pub supported_mime_types: Vec<String>,
}

/// File transmission method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransmissionMethod {
    /// Inline Base64 encoding (file embedded in request body).
    InlineBase64,
    /// File upload API, returning a file_id referenced in the request.
    /// Actual HTTP upload is performed by the external LLM client;
    /// pawbun-files only hints thresholds via `constraints()`.
    UploadApi,
    /// Pass URL through directly (file source is already a URL and provider supports it).
    UrlReference,
}

/// Provider formatting trait.
///
/// Converts `MediaContent` into JSON fragments required by specific LLM Provider APIs.
///
/// **Boundary**: this trait only handles "content formatting", not HTTP upload logic.
pub trait ProviderFormat: std::fmt::Debug + Send + Sync {
    /// Provider name (e.g. `"openai"`, `"anthropic"`).
    fn provider_name(&self) -> &str;

    /// Converts a single `MediaContent` into a Provider content block.
    fn format_content(&self, content: &MediaContent) -> Result<Value, FormatError>;

    /// Formats a `File` into a Provider content block (high-level convenience).
    ///
    /// Internally:
    /// 1. If `file.source` is `Url` and `format_reference` returns `Some`, use the reference format directly.
    /// 2. Otherwise load the file via `loader`, then call `format_content`.
    fn format_file(&self, file: &File, loader: &dyn FileLoader) -> Result<Value, FormatError> {
        if let Some(reference) = self.format_reference(file) {
            return Ok(reference);
        }
        let loaded = loader
            .load(file)
            .map_err(|e| FormatError::Load(e.to_string()))?;
        self.format_content(&loaded.content)
    }

    /// Converts an unloaded `File` into a Provider-supported reference format (e.g. URL).
    ///
    /// Only returns `Some(Value)` when `FileSource::Url` and the provider supports URL references.
    /// Otherwise returns `None`; caller must `load` then call `format_content`.
    fn format_reference(&self, file: &File) -> Option<Value>;

    /// Returns constraints for a specific media type.
    fn constraints(&self, media_type: MediaType) -> ProviderConstraints;

    /// Automatically selects the optimal transmission method based on file size and source.
    ///
    /// `loaded_size` is the actual loaded content size in bytes, preferred over
    /// `file.metadata.size_bytes` because the latter may be inaccurate or empty.
    fn select_method(&self, file: &File, loaded_size: u64) -> TransmissionMethod {
        if matches!(file.source, crate::file::FileSource::Url { .. })
            && self.format_reference(file).is_some()
        {
            return TransmissionMethod::UrlReference;
        }

        let constraints = self.constraints(file.media_type.unwrap_or(MediaType::Binary));
        if loaded_size <= constraints.max_size_bytes {
            TransmissionMethod::InlineBase64
        } else {
            TransmissionMethod::UploadApi
        }
    }
}

// ------------------------------------------------------------------------------
// Anthropic ProviderFormat implementation
// ------------------------------------------------------------------------------

/// Anthropic Claude API formatter.
#[derive(Debug, Clone, Default)]
pub struct AnthropicFormat;

impl ProviderFormat for AnthropicFormat {
    fn provider_name(&self) -> &str {
        "anthropic"
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(content.media_type = ?content.media_type())))]
    fn format_content(&self, content: &MediaContent) -> Result<Value, FormatError> {
        match content {
            MediaContent::Image(img) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&img.bytes);
                let mime = img.format.mime_type();
                Ok(serde_json::json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": mime,
                        "data": b64
                    }
                }))
            }
            MediaContent::Text(txt) => Ok(serde_json::json!({
                "type": "text",
                "text": txt.text
            })),
            MediaContent::Pdf(pdf) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&pdf.bytes);
                Ok(serde_json::json!({
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": "application/pdf",
                        "data": b64
                    }
                }))
            }
            MediaContent::Audio(_) | MediaContent::Video(_) => {
                Err(FormatError::UnsupportedMediaType {
                    provider: "anthropic".into(),
                    media_type: content.media_type(),
                })
            }
            MediaContent::Binary(bin) => {
                // Anthropic supports PDF via document type and Image via base64 source.
                match bin.guessed_type {
                    Some(MediaType::Pdf) => {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bin.bytes);
                        Ok(serde_json::json!({
                            "type": "document",
                            "source": {
                                "type": "base64",
                                "media_type": "application/pdf",
                                "data": b64
                            }
                        }))
                    }
                    Some(MediaType::Image(fmt)) => {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bin.bytes);
                        let mime = fmt.mime_type();
                        Ok(serde_json::json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": mime,
                                "data": b64
                            }
                        }))
                    }
                    _ => Err(FormatError::UnsupportedMediaType {
                        provider: "anthropic".into(),
                        media_type: content.media_type(),
                    }),
                }
            }
        }
    }

    fn format_reference(&self, file: &File) -> Option<Value> {
        match &file.source {
            crate::file::FileSource::Url { url } => {
                // Anthropic only supports image URLs directly; other types must be loaded inline.
                if matches!(file.media_type, Some(MediaType::Image(_))) {
                    Some(serde_json::json!({
                        "type": "image",
                        "source": {
                            "type": "url",
                            "url": url
                        }
                    }))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn constraints(&self, media_type: MediaType) -> ProviderConstraints {
        match media_type {
            MediaType::Image(_) => ProviderConstraints {
                max_size_bytes: 5 * 1024 * 1024,
                max_files_per_request: 20,
                supported_mime_types: vec![
                    "image/png".into(),
                    "image/jpeg".into(),
                    "image/webp".into(),
                    "image/gif".into(),
                ],
            },
            MediaType::Pdf => ProviderConstraints {
                max_size_bytes: 32 * 1024 * 1024,
                max_files_per_request: 1,
                supported_mime_types: vec!["application/pdf".into()],
            },
            MediaType::Text => ProviderConstraints {
                max_size_bytes: u64::MAX,
                max_files_per_request: usize::MAX,
                supported_mime_types: vec!["text/plain".into()],
            },
            _ => ProviderConstraints {
                max_size_bytes: u64::MAX,
                max_files_per_request: usize::MAX,
                supported_mime_types: vec![],
            },
        }
    }
}

// ------------------------------------------------------------------------------
// Gemini ProviderFormat implementation
// ------------------------------------------------------------------------------

/// Google Gemini 1.5+ API formatter.
#[derive(Debug, Clone, Default)]
pub struct GeminiFormat;

impl ProviderFormat for GeminiFormat {
    fn provider_name(&self) -> &str {
        "gemini"
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(content.media_type = ?content.media_type())))]
    fn format_content(&self, content: &MediaContent) -> Result<Value, FormatError> {
        match content {
            MediaContent::Text(txt) => Ok(serde_json::json!({
                "text": txt.text
            })),
            MediaContent::Image(img) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&img.bytes);
                let mime = img.format.mime_type();
                Ok(serde_json::json!({
                    "inline_data": {
                        "mime_type": mime,
                        "data": b64
                    }
                }))
            }
            MediaContent::Pdf(pdf) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&pdf.bytes);
                Ok(serde_json::json!({
                    "inline_data": {
                        "mime_type": "application/pdf",
                        "data": b64
                    }
                }))
            }
            MediaContent::Audio(audio) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&audio.bytes);
                let mime = audio.format.mime_type();
                Ok(serde_json::json!({
                    "inline_data": {
                        "mime_type": mime,
                        "data": b64
                    }
                }))
            }
            MediaContent::Video(video) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&video.bytes);
                let mime = video.format.mime_type();
                Ok(serde_json::json!({
                    "inline_data": {
                        "mime_type": mime,
                        "data": b64
                    }
                }))
            }
            MediaContent::Binary(bin) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bin.bytes);
                let mime = bin
                    .guessed_type
                    .and_then(|t| t.mime_type())
                    .unwrap_or("application/octet-stream");
                Ok(serde_json::json!({
                    "inline_data": {
                        "mime_type": mime,
                        "data": b64
                    }
                }))
            }
        }
    }

    fn format_reference(&self, _file: &File) -> Option<Value> {
        // Gemini supports file URI references via fileData, but that requires
        // prior upload. For simplicity, Beta stage always loads inline.
        None
    }

    fn constraints(&self, media_type: MediaType) -> ProviderConstraints {
        match media_type {
            MediaType::Image(_) => ProviderConstraints {
                max_size_bytes: 20 * 1024 * 1024,
                max_files_per_request: 10,
                supported_mime_types: vec![
                    "image/png".into(),
                    "image/jpeg".into(),
                    "image/webp".into(),
                    "image/gif".into(),
                ],
            },
            MediaType::Pdf => ProviderConstraints {
                max_size_bytes: 20 * 1024 * 1024,
                max_files_per_request: 1,
                supported_mime_types: vec!["application/pdf".into()],
            },
            MediaType::Audio(_) => ProviderConstraints {
                max_size_bytes: 20 * 1024 * 1024,
                max_files_per_request: 1,
                supported_mime_types: vec![
                    "audio/mpeg".into(),
                    "audio/wav".into(),
                    "audio/ogg".into(),
                ],
            },
            MediaType::Video(_) => ProviderConstraints {
                max_size_bytes: 20 * 1024 * 1024,
                max_files_per_request: 1,
                supported_mime_types: vec!["video/mp4".into(), "video/webm".into()],
            },
            MediaType::Text => ProviderConstraints {
                max_size_bytes: u64::MAX,
                max_files_per_request: usize::MAX,
                supported_mime_types: vec!["text/plain".into()],
            },
            _ => ProviderConstraints {
                max_size_bytes: u64::MAX,
                max_files_per_request: usize::MAX,
                supported_mime_types: vec![],
            },
        }
    }
}

// ------------------------------------------------------------------------------
// Azure OpenAI ProviderFormat implementation
// ------------------------------------------------------------------------------

/// Azure OpenAI API formatter.
#[derive(Debug, Clone, Default)]
pub struct AzureOpenAiFormat;

impl ProviderFormat for AzureOpenAiFormat {
    fn provider_name(&self) -> &str {
        "azure_openai"
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(content.media_type = ?content.media_type())))]
    fn format_content(&self, content: &MediaContent) -> Result<Value, FormatError> {
        match content {
            MediaContent::Image(img) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&img.bytes);
                let mime = img.format.mime_type();
                Ok(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{mime};base64,{b64}")
                    }
                }))
            }
            MediaContent::Text(txt) => Ok(serde_json::json!({
                "type": "text",
                "text": txt.text
            })),
            MediaContent::Audio(audio) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&audio.bytes);
                let format_str = match audio.format {
                    crate::media::AudioFormat::Wav => "wav",
                    crate::media::AudioFormat::Mp3 => "mp3",
                    _ => "wav",
                };
                Ok(serde_json::json!({
                    "type": "input_audio",
                    "input_audio": {
                        "data": b64,
                        "format": format_str
                    }
                }))
            }
            MediaContent::Pdf(_) | MediaContent::Video(_) => {
                Err(FormatError::UnsupportedMediaType {
                    provider: "azure_openai".into(),
                    media_type: content.media_type(),
                })
            }
            MediaContent::Binary(bin) => match bin.guessed_type {
                Some(MediaType::Image(fmt)) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bin.bytes);
                    let mime = fmt.mime_type();
                    Ok(serde_json::json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{mime};base64,{b64}")
                        }
                    }))
                }
                Some(MediaType::Audio(_)) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bin.bytes);
                    Ok(serde_json::json!({
                        "type": "input_audio",
                        "input_audio": {
                            "data": b64,
                            "format": "wav"
                        }
                    }))
                }
                _ => Err(FormatError::UnsupportedMediaType {
                    provider: "azure_openai".into(),
                    media_type: content.media_type(),
                }),
            },
        }
    }

    fn format_reference(&self, file: &File) -> Option<Value> {
        match &file.source {
            crate::file::FileSource::Url { url } => Some(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": url }
            })),
            _ => None,
        }
    }

    fn constraints(&self, media_type: MediaType) -> ProviderConstraints {
        match media_type {
            MediaType::Image(_) => ProviderConstraints {
                max_size_bytes: 20 * 1024 * 1024,
                max_files_per_request: 10,
                supported_mime_types: vec![
                    "image/png".into(),
                    "image/jpeg".into(),
                    "image/webp".into(),
                    "image/gif".into(),
                ],
            },
            MediaType::Audio(_) => ProviderConstraints {
                max_size_bytes: 20 * 1024 * 1024,
                max_files_per_request: 1,
                supported_mime_types: vec!["audio/wav".into(), "audio/mpeg".into()],
            },
            MediaType::Text => ProviderConstraints {
                max_size_bytes: u64::MAX,
                max_files_per_request: usize::MAX,
                supported_mime_types: vec!["text/plain".into()],
            },
            _ => ProviderConstraints {
                max_size_bytes: u64::MAX,
                max_files_per_request: usize::MAX,
                supported_mime_types: vec![],
            },
        }
    }
}

// ------------------------------------------------------------------------------
// OpenAI ProviderFormat implementation
// ------------------------------------------------------------------------------

/// OpenAI chat completions / responses API formatter.
#[derive(Debug, Clone, Default)]
pub struct OpenAiFormat;

impl ProviderFormat for OpenAiFormat {
    fn provider_name(&self) -> &str {
        "openai"
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(content.media_type = ?content.media_type())))]
    fn format_content(&self, content: &MediaContent) -> Result<Value, FormatError> {
        match content {
            MediaContent::Image(img) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&img.bytes);
                let mime = img.format.mime_type();
                Ok(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{mime};base64,{b64}")
                    }
                }))
            }
            MediaContent::Text(txt) => Ok(serde_json::json!({
                "type": "text",
                "text": txt.text
            })),
            MediaContent::Pdf(_) | MediaContent::Audio(_) | MediaContent::Video(_) => {
                Err(FormatError::UnsupportedMediaType {
                    provider: "openai".into(),
                    media_type: content.media_type(),
                })
            }
            MediaContent::Binary(bin) => {
                // OpenAI chat does not support generic binary; try to treat as image if guessed.
                if let Some(MediaType::Image(fmt)) = bin.guessed_type {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bin.bytes);
                    let mime = fmt.mime_type();
                    Ok(serde_json::json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{mime};base64,{b64}")
                        }
                    }))
                } else {
                    Err(FormatError::UnsupportedMediaType {
                        provider: "openai".into(),
                        media_type: content.media_type(),
                    })
                }
            }
        }
    }

    fn format_reference(&self, file: &File) -> Option<Value> {
        match &file.source {
            crate::file::FileSource::Url { url } => Some(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": url }
            })),
            _ => None,
        }
    }

    fn constraints(&self, media_type: MediaType) -> ProviderConstraints {
        match media_type {
            MediaType::Image(_) => ProviderConstraints {
                max_size_bytes: 20 * 1024 * 1024,
                max_files_per_request: 10,
                supported_mime_types: vec![
                    "image/png".into(),
                    "image/jpeg".into(),
                    "image/webp".into(),
                    "image/gif".into(),
                ],
            },
            MediaType::Text => ProviderConstraints {
                max_size_bytes: u64::MAX,
                max_files_per_request: usize::MAX,
                supported_mime_types: vec!["text/plain".into()],
            },
            _ => ProviderConstraints {
                max_size_bytes: u64::MAX,
                max_files_per_request: usize::MAX,
                supported_mime_types: vec![],
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{BinaryContent, ImageContent, TextContent};
    use crate::media::ImageFormat;
    use bytes::Bytes;

    #[test]
    fn test_anthropic_format_text() {
        let fmt = AnthropicFormat;
        let content = MediaContent::Text(TextContent {
            text: "hello".into(),
            encoding: None,
        });
        let val = fmt.format_content(&content).unwrap();
        assert_eq!(val["type"], "text");
        assert_eq!(val["text"], "hello");
    }

    #[test]
    fn test_anthropic_format_image() {
        let fmt = AnthropicFormat;
        let content = MediaContent::Image(ImageContent {
            bytes: Bytes::from_static(b"fake"),
            format: ImageFormat::Png,
            width: None,
            height: None,
        });
        let val = fmt.format_content(&content).unwrap();
        assert_eq!(val["type"], "image");
        assert_eq!(val["source"]["type"], "base64");
        assert_eq!(val["source"]["media_type"], "image/png");
        assert!(!val["source"]["data"].as_str().unwrap().is_empty());
    }

    #[test]
    fn test_anthropic_format_binary_pdf() {
        let fmt = AnthropicFormat;
        let content = MediaContent::Binary(BinaryContent {
            bytes: Bytes::from_static(b"pdf"),
            guessed_type: Some(MediaType::Pdf),
        });
        let val = fmt.format_content(&content).unwrap();
        assert_eq!(val["type"], "document");
        assert_eq!(val["source"]["media_type"], "application/pdf");
    }

    #[test]
    fn test_anthropic_format_binary_unsupported() {
        let fmt = AnthropicFormat;
        let content = MediaContent::Binary(BinaryContent {
            bytes: Bytes::from_static(b"raw"),
            guessed_type: Some(MediaType::Binary),
        });
        let result = fmt.format_content(&content);
        assert!(
            matches!(result, Err(FormatError::UnsupportedMediaType { provider, .. }) if provider == "anthropic")
        );
    }

    #[test]
    fn test_anthropic_format_reference_url() {
        let fmt = AnthropicFormat;
        let file = crate::file::File::from_url("https://example.com/img.png");
        let val = fmt.format_reference(&file).unwrap();
        assert_eq!(val["type"], "image");
        assert_eq!(val["source"]["type"], "url");
        assert_eq!(val["source"]["url"], "https://example.com/img.png");
    }

    #[test]
    fn test_anthropic_constraints() {
        let fmt = AnthropicFormat;
        let c = fmt.constraints(MediaType::Image(ImageFormat::Png));
        assert_eq!(c.max_size_bytes, 5 * 1024 * 1024);
        assert_eq!(c.max_files_per_request, 20);
        assert!(c.supported_mime_types.contains(&"image/png".to_string()));

        let c_pdf = fmt.constraints(MediaType::Pdf);
        assert_eq!(c_pdf.max_size_bytes, 32 * 1024 * 1024);
        assert_eq!(c_pdf.max_files_per_request, 1);
    }

    #[test]
    fn test_openai_format_text() {
        let fmt = OpenAiFormat;
        let content = MediaContent::Text(TextContent {
            text: "hello".into(),
            encoding: None,
        });
        let val = fmt.format_content(&content).unwrap();
        assert_eq!(val["type"], "text");
        assert_eq!(val["text"], "hello");
    }

    #[test]
    fn test_openai_format_image() {
        let fmt = OpenAiFormat;
        let content = MediaContent::Image(ImageContent {
            bytes: Bytes::from_static(b"fake"),
            format: ImageFormat::Png,
            width: None,
            height: None,
        });
        let val = fmt.format_content(&content).unwrap();
        assert_eq!(val["type"], "image_url");
        let url = val["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn test_openai_format_binary_unsupported() {
        let fmt = OpenAiFormat;
        let content = MediaContent::Binary(BinaryContent {
            bytes: Bytes::from_static(b"raw"),
            guessed_type: Some(MediaType::Binary),
        });
        let result = fmt.format_content(&content);
        assert!(
            matches!(result, Err(FormatError::UnsupportedMediaType { provider, .. }) if provider == "openai")
        );
    }

    #[test]
    fn test_openai_format_reference_url() {
        let fmt = OpenAiFormat;
        let file = crate::file::File::from_url("https://example.com/img.png");
        let val = fmt.format_reference(&file).unwrap();
        assert_eq!(val["type"], "image_url");
        assert_eq!(val["image_url"]["url"], "https://example.com/img.png");
    }

    #[test]
    fn test_openai_constraints() {
        let fmt = OpenAiFormat;
        let c = fmt.constraints(MediaType::Image(ImageFormat::Png));
        assert_eq!(c.max_size_bytes, 20 * 1024 * 1024);
        assert_eq!(c.max_files_per_request, 10);
        assert!(c.supported_mime_types.contains(&"image/png".to_string()));
    }

    #[test]
    fn test_select_method_url() {
        let fmt = OpenAiFormat;
        let file = crate::file::File::from_url("https://example.com/img.png");
        assert_eq!(
            fmt.select_method(&file, 100),
            TransmissionMethod::UrlReference
        );
    }

    #[test]
    fn test_select_method_inline() {
        let fmt = OpenAiFormat;
        let file = crate::file::File::from_path("img.png");
        assert_eq!(
            fmt.select_method(&file, 1024),
            TransmissionMethod::InlineBase64
        );
    }

    #[test]
    fn test_select_method_upload() {
        let fmt = OpenAiFormat;
        let file = crate::file::File::from_path("img.png");
        assert_eq!(
            fmt.select_method(&file, 100 * 1024 * 1024),
            TransmissionMethod::UploadApi
        );
    }

    #[test]
    fn test_gemini_format_text() {
        let fmt = GeminiFormat;
        let content = MediaContent::Text(TextContent {
            text: "hello".into(),
            encoding: None,
        });
        let val = fmt.format_content(&content).unwrap();
        assert_eq!(val["text"], "hello");
    }

    #[test]
    fn test_gemini_format_image() {
        let fmt = GeminiFormat;
        let content = MediaContent::Image(ImageContent {
            bytes: Bytes::from_static(b"fake"),
            format: ImageFormat::Png,
            width: None,
            height: None,
        });
        let val = fmt.format_content(&content).unwrap();
        assert_eq!(val["inline_data"]["mime_type"], "image/png");
        assert!(!val["inline_data"]["data"].as_str().unwrap().is_empty());
    }

    #[test]
    fn test_gemini_format_pdf() {
        let fmt = GeminiFormat;
        let content = MediaContent::Pdf(crate::content::PdfContent {
            bytes: Bytes::from_static(b"pdf"),
            pages: None,
            text_preview: None,
        });
        let val = fmt.format_content(&content).unwrap();
        assert_eq!(val["inline_data"]["mime_type"], "application/pdf");
    }

    #[test]
    fn test_gemini_constraints() {
        let fmt = GeminiFormat;
        let c = fmt.constraints(MediaType::Image(ImageFormat::Png));
        assert_eq!(c.max_size_bytes, 20 * 1024 * 1024);
        let c_audio = fmt.constraints(MediaType::Audio(crate::media::AudioFormat::Mp3));
        assert_eq!(c_audio.max_size_bytes, 20 * 1024 * 1024);
    }

    #[test]
    fn test_azure_format_text() {
        let fmt = AzureOpenAiFormat;
        let content = MediaContent::Text(TextContent {
            text: "hello".into(),
            encoding: None,
        });
        let val = fmt.format_content(&content).unwrap();
        assert_eq!(val["type"], "text");
        assert_eq!(val["text"], "hello");
    }

    #[test]
    fn test_azure_format_image() {
        let fmt = AzureOpenAiFormat;
        let content = MediaContent::Image(ImageContent {
            bytes: Bytes::from_static(b"fake"),
            format: ImageFormat::Jpeg,
            width: None,
            height: None,
        });
        let val = fmt.format_content(&content).unwrap();
        assert_eq!(val["type"], "image_url");
        let url = val["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn test_azure_format_audio() {
        let fmt = AzureOpenAiFormat;
        let content = MediaContent::Audio(crate::content::AudioContent {
            bytes: Bytes::from_static(b"audio"),
            format: crate::media::AudioFormat::Wav,
            duration: None,
            sample_rate: None,
        });
        let val = fmt.format_content(&content).unwrap();
        assert_eq!(val["type"], "input_audio");
        assert_eq!(val["input_audio"]["format"], "wav");
    }

    #[test]
    fn test_azure_format_reference_url() {
        let fmt = AzureOpenAiFormat;
        let file = crate::file::File::from_url("https://example.com/img.png");
        let val = fmt.format_reference(&file).unwrap();
        assert_eq!(val["type"], "image_url");
        assert_eq!(val["image_url"]["url"], "https://example.com/img.png");
    }

    #[test]
    fn test_azure_constraints() {
        let fmt = AzureOpenAiFormat;
        let c = fmt.constraints(MediaType::Image(ImageFormat::Png));
        assert_eq!(c.max_size_bytes, 20 * 1024 * 1024);
        let c_audio = fmt.constraints(MediaType::Audio(crate::media::AudioFormat::Wav));
        assert!(c_audio
            .supported_mime_types
            .contains(&"audio/wav".to_string()));
    }
}
