//! Unified multimodal content representation.

use std::borrow::Cow;

use bytes::Bytes;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::media::{AudioFormat, ImageFormat, MediaType, VideoFormat};

/// Custom serde module for Base64 encoding/decoding of `bytes::Bytes`.
///
/// Serializes raw bytes as a Base64 string and deserializes a Base64 string back into `Bytes`.
pub mod base64_bytes {
    use super::*;
    use base64::Engine;

    /// Serializes `Bytes` as a Base64-encoded string.
    pub fn serialize<S>(bytes: &Bytes, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        serializer.serialize_str(&encoded)
    }

    /// 最大允许的 Base64 字符串长度（约 140 MiB，对应约 105 MiB 原始数据）。
    const MAX_BASE64_LEN: usize = 140 * 1024 * 1024;

    /// Deserializes a Base64-encoded string into `Bytes`.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: Cow<'de, str> = Deserialize::deserialize(deserializer)?;
        if s.len() > MAX_BASE64_LEN {
            return Err(serde::de::Error::custom(format!(
                "base64 string exceeds maximum length of {} bytes",
                MAX_BASE64_LEN
            )));
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)?;
        Ok(Bytes::from(decoded))
    }
}

/// Unified multimodal content representation.
///
/// When serialized, all `bytes::Bytes` fields are encoded as Base64 strings
/// via the internal `base64_bytes` module to avoid JSON array bloat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MediaContent {
    /// Textual content.
    Text(TextContent),
    /// Image content, carrying pixel dimensions and format.
    Image(ImageContent),
    /// PDF document content.
    Pdf(PdfContent),
    /// Audio content, carrying duration and sample rate.
    Audio(AudioContent),
    /// Video content, carrying duration and thumbnail.
    Video(VideoContent),
    /// Unknown or unparsed binary content.
    Binary(BinaryContent),
}

/// Text content with optional encoding metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextContent {
    /// Text content.
    pub text: String,
    /// Text encoding (usually UTF-8).
    pub encoding: Option<String>,
}

/// Image content with dimensions and format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageContent {
    /// Raw image bytes (serialized as Base64 string; internally reference-counted, cheap clone).
    #[serde(with = "base64_bytes")]
    pub bytes: Bytes,
    /// Image format (PNG, JPEG, etc.).
    pub format: ImageFormat,
    /// Image width in pixels, if known.
    pub width: Option<u32>,
    /// Image height in pixels, if known.
    pub height: Option<u32>,
}

/// PDF document content with optional page count and text preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PdfContent {
    /// Raw PDF bytes (serialized as Base64 string; internally reference-counted, cheap clone).
    #[serde(with = "base64_bytes")]
    pub bytes: Bytes,
    /// Number of pages (if parsed).
    pub pages: Option<usize>,
    /// Text preview (first N characters), for debugging and logging.
    pub text_preview: Option<String>,
}

/// Audio content with duration and sample rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioContent {
    /// Raw audio bytes (serialized as Base64 string; internally reference-counted, cheap clone).
    #[serde(with = "base64_bytes")]
    pub bytes: Bytes,
    /// Audio format (MP3, WAV, etc.).
    pub format: AudioFormat,
    /// Audio duration in seconds.
    pub duration: Option<f64>,
    /// Sample rate in Hz.
    pub sample_rate: Option<u32>,
}

/// Video content with duration and optional thumbnail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoContent {
    /// Raw video bytes (serialized as Base64 string; internally reference-counted, cheap clone).
    #[serde(with = "base64_bytes")]
    pub bytes: Bytes,
    /// Video format (MP4, WebM, etc.).
    pub format: VideoFormat,
    /// Video duration in seconds.
    pub duration: Option<f64>,
    /// Video thumbnail (first frame as image, optional).
    pub thumbnail: Option<ImageContent>,
}

/// Unknown or unrecognized binary content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BinaryContent {
    /// Raw binary bytes (serialized as Base64 string; internally reference-counted, cheap clone).
    #[serde(with = "base64_bytes")]
    pub bytes: Bytes,
    /// If known, a user-provided media type guess.
    pub guessed_type: Option<MediaType>,
}

impl MediaContent {
    /// Returns the raw bytes if available.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            MediaContent::Text(_) => None,
            MediaContent::Image(img) => Some(&img.bytes),
            MediaContent::Pdf(pdf) => Some(&pdf.bytes),
            MediaContent::Audio(audio) => Some(&audio.bytes),
            MediaContent::Video(video) => Some(&video.bytes),
            MediaContent::Binary(bin) => Some(&bin.bytes),
        }
    }

    /// Returns the text content (only for the `Text` variant).
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MediaContent::Text(txt) => Some(&txt.text),
            _ => None,
        }
    }

    /// Returns the media type of this content.
    pub fn media_type(&self) -> MediaType {
        match self {
            MediaContent::Text(_) => MediaType::Text,
            MediaContent::Image(img) => MediaType::Image(img.format),
            MediaContent::Pdf(_) => MediaType::Pdf,
            MediaContent::Audio(audio) => MediaType::Audio(audio.format),
            MediaContent::Video(video) => MediaType::Video(video.format),
            MediaContent::Binary(bin) => bin.guessed_type.unwrap_or(MediaType::Binary),
        }
    }

    /// Returns the content size in bytes.
    pub fn size_bytes(&self) -> u64 {
        match self {
            MediaContent::Text(txt) => txt.text.len() as u64,
            MediaContent::Image(img) => img.bytes.len() as u64,
            MediaContent::Pdf(pdf) => pdf.bytes.len() as u64,
            MediaContent::Audio(audio) => audio.bytes.len() as u64,
            MediaContent::Video(video) => video.bytes.len() as u64,
            MediaContent::Binary(bin) => bin.bytes.len() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::ImageFormat;

    #[test]
    fn test_media_content_text() {
        let content = MediaContent::Text(TextContent {
            text: "hello".into(),
            encoding: Some("utf-8".into()),
        });
        assert_eq!(content.as_text(), Some("hello"));
        assert_eq!(content.as_bytes(), None);
        assert_eq!(content.media_type(), MediaType::Text);
        assert_eq!(content.size_bytes(), 5);
    }

    #[test]
    fn test_media_content_image() {
        let bytes = Bytes::from_static(b"fake_image");
        let content = MediaContent::Image(ImageContent {
            bytes: bytes.clone(),
            format: ImageFormat::Png,
            width: Some(100),
            height: Some(200),
        });
        assert_eq!(content.as_text(), None);
        assert_eq!(content.as_bytes(), Some(b"fake_image".as_slice()));
        assert_eq!(content.media_type(), MediaType::Image(ImageFormat::Png));
        assert_eq!(content.size_bytes(), 10);
    }

    #[test]
    fn test_media_content_binary() {
        let bytes = Bytes::from_static(b"raw_data");
        let content = MediaContent::Binary(BinaryContent {
            bytes: bytes.clone(),
            guessed_type: Some(MediaType::Image(ImageFormat::Jpeg)),
        });
        assert_eq!(content.media_type(), MediaType::Image(ImageFormat::Jpeg));
        assert_eq!(content.size_bytes(), 8);
    }

    #[test]
    fn test_serde_roundtrip_text() {
        let content = MediaContent::Text(TextContent {
            text: "hello world".into(),
            encoding: Some("utf-8".into()),
        });
        let json = serde_json::to_string(&content).unwrap();
        let decoded: MediaContent = serde_json::from_str(&json).unwrap();
        assert_eq!(content, decoded);
    }

    #[test]
    fn test_serde_roundtrip_image() {
        let bytes = Bytes::from_static(b"fake_image_data");
        let content = MediaContent::Image(ImageContent {
            bytes,
            format: ImageFormat::Png,
            width: None,
            height: None,
        });
        let json = serde_json::to_string(&content).unwrap();
        // Ensure bytes are Base64 encoded, not a numeric array
        assert!(!json.contains("fake_image_data"));
        assert!(json.contains("ZmFrZV9pbWFnZV9kYXRh")); // base64 fragment of "fake_image_data"
        let decoded: MediaContent = serde_json::from_str(&json).unwrap();
        assert_eq!(content, decoded);
    }

    #[test]
    fn test_base64_bytes_module() {
        let original = Bytes::from_static(b"hello");
        // When using the module directly via wrapper, we test via ImageContent above.
        // This test verifies standalone base64 behavior through serde_json::Value.
        let val = serde_json::json!("aGVsbG8=");
        let decoded: Bytes = base64_bytes::deserialize(val).unwrap();
        assert_eq!(decoded, original);
    }
}
