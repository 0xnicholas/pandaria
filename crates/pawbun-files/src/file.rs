//! Unified file handle and source abstractions.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::constraints::FileConstraints;
use crate::media::MediaType;

/// Unified file handle representing a multimodal file to be loaded or already loaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct File {
    /// Template reference key (e.g. `"sales_chart"`), used by upper-layer
    /// agent/workflow template systems. If `None`, not referenced by templates.
    pub key: Option<String>,

    /// File source (local path / URL / in-memory bytes).
    pub source: FileSource,

    /// Media type. If `None`, auto-detected by `FileLoader`.
    pub media_type: Option<MediaType>,

    /// User-specified constraints (size, format whitelist, etc.).
    pub constraints: FileConstraints,

    /// File metadata (file name, MIME type, modification time, etc.).
    /// May be preset by the user (e.g. `from_bytes` auto-fills `name`).
    pub metadata: FileMetadata,
}

impl File {
    /// Creates a file from a local path, auto-inferring media type from extension.
    /// Does not validate whether the path exists or is readable; failures are deferred
    /// to `FileLoader::load()`.
    pub fn from_path<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref();
        let media_type = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(MediaType::from_extension);

        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());

        Self {
            key: None,
            source: FileSource::Local {
                path: path.to_path_buf(),
            },
            media_type,
            constraints: FileConstraints::default(),
            metadata: FileMetadata {
                name,
                mime_type: media_type.and_then(|t| t.mime_type().map(|m| m.to_string())),
                size_bytes: None,
                modified_at: None,
            },
        }
    }

    /// Creates a file from a URL.
    /// Does not validate whether the URL is reachable; failures are deferred to `FileLoader::load()`.
    pub fn from_url(url: impl Into<String>) -> Self {
        let url = url.into();
        let name = url.rsplit('/').next().map(|s| s.to_string());
        let media_type = name.as_ref().and_then(|n| {
            Path::new(n)
                .extension()
                .and_then(|e| e.to_str())
                .and_then(MediaType::from_extension)
        });

        Self {
            key: None,
            source: FileSource::Url { url },
            media_type,
            constraints: FileConstraints::default(),
            metadata: FileMetadata {
                name,
                mime_type: media_type.and_then(|t| t.mime_type().map(|m| m.to_string())),
                size_bytes: None,
                modified_at: None,
            },
        }
    }

    /// Creates a file from in-memory bytes.
    /// `hint` is a file name or extension used to infer media type and fill `metadata.name`
    /// (e.g. `"chart.png"` → `media_type=Image(Png)`, `metadata.name="chart.png"`).
    pub fn from_bytes(bytes: Bytes, hint: &str) -> Self {
        let media_type = Path::new(hint)
            .extension()
            .and_then(|e| e.to_str())
            .and_then(MediaType::from_extension);

        let size_bytes = Some(bytes.len() as u64);

        Self {
            key: None,
            source: FileSource::Bytes { data: bytes },
            media_type,
            constraints: FileConstraints::default(),
            metadata: FileMetadata {
                name: Some(hint.to_string()),
                mime_type: media_type.and_then(|t| t.mime_type().map(|m| m.to_string())),
                size_bytes,
                modified_at: None,
            },
        }
    }

    /// Sets the template reference key.
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Sets the media type (overrides auto-detection).
    pub fn with_media_type(mut self, ty: MediaType) -> Self {
        self.media_type = Some(ty);
        self.metadata.mime_type = ty.mime_type().map(|m| m.to_string());
        self
    }

    /// Sets constraint conditions.
    pub fn with_constraints(mut self, c: FileConstraints) -> Self {
        self.constraints = c;
        self
    }
}

/// File data source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FileSource {
    /// Local filesystem path.
    Local {
        /// Local file path.
        path: PathBuf,
    },
    /// Remote URL (HTTP/HTTPS).
    Url {
        /// Remote URL string.
        url: String,
    },
    /// In-memory byte data (reference-counted, cheap clone).
    Bytes {
        /// Raw byte data (serialized as Base64).
        #[serde(with = "crate::content::base64_bytes")]
        data: Bytes,
    },
}

impl FileSource {
    /// Attempts to return a string that can be used to identify this source
    /// (file name or last segment of URL).
    pub fn hint(&self) -> Option<String> {
        match self {
            FileSource::Local { path } => {
                path.file_name().map(|n| n.to_string_lossy().into_owned())
            }
            FileSource::Url { url } => url.rsplit('/').next().map(|s| s.to_string()),
            FileSource::Bytes { .. } => None,
        }
    }
}

/// File metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileMetadata {
    /// Original file name (without path).
    pub name: Option<String>,
    /// MIME type, e.g. `image/png`.
    pub mime_type: Option<String>,
    /// File size in bytes. Using `u64` to support 32-bit systems and large files (>4GB).
    pub size_bytes: Option<u64>,
    /// Last modification time.
    pub modified_at: Option<std::time::SystemTime>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::ImageFormat;

    #[test]
    fn test_from_path() {
        let file = File::from_path("./report.png");
        assert_eq!(file.media_type, Some(MediaType::Image(ImageFormat::Png)));
        assert_eq!(file.metadata.name, Some("report.png".into()));
        assert!(matches!(file.source, FileSource::Local { .. }));
    }

    #[test]
    fn test_from_url() {
        let file = File::from_url("https://example.com/chart.jpg");
        assert_eq!(file.media_type, Some(MediaType::Image(ImageFormat::Jpeg)));
        assert_eq!(file.metadata.name, Some("chart.jpg".into()));
        assert!(matches!(file.source, FileSource::Url { .. }));
    }

    #[test]
    fn test_from_bytes() {
        let data = Bytes::from_static(b"hello");
        let file = File::from_bytes(data.clone(), "note.txt");
        assert_eq!(file.media_type, Some(MediaType::Text));
        assert_eq!(file.metadata.name, Some("note.txt".into()));
        assert_eq!(file.metadata.size_bytes, Some(5));
        assert!(matches!(file.source, FileSource::Bytes { .. }));
    }

    #[test]
    fn test_builder_chain() {
        let file = File::from_path("img.png")
            .with_key("hero_image")
            .with_media_type(MediaType::Image(ImageFormat::Png));
        assert_eq!(file.key, Some("hero_image".into()));
        assert_eq!(file.media_type, Some(MediaType::Image(ImageFormat::Png)));
    }

    #[test]
    fn test_source_hint() {
        let local = FileSource::Local {
            path: PathBuf::from("/tmp/data.txt"),
        };
        assert_eq!(local.hint(), Some("data.txt".into()));

        let url = FileSource::Url {
            url: "https://example.com/file.pdf".into(),
        };
        assert_eq!(url.hint(), Some("file.pdf".into()));

        let bytes = FileSource::Bytes {
            data: Bytes::from_static(b"x"),
        };
        assert_eq!(bytes.hint(), None);
    }

    #[test]
    fn test_bytes_source_serde_base64() {
        let file = File::from_bytes(Bytes::from_static(b"hello"), "data.bin");
        let json = serde_json::to_string(&file).unwrap();
        // Ensure bytes are Base64 encoded, not a numeric array
        assert!(!json.contains("[104,101,108,108,111]"));
        assert!(json.contains("aGVsbG8=")); // base64 of "hello"

        let decoded: File = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, file);
    }
}
