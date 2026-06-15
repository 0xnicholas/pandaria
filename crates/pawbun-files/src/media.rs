//! Media type definitions and MIME type inference.

use serde::{Deserialize, Serialize};

/// Media type enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MediaType {
    /// Plain text or structured text (Markdown, JSON, CSV, etc.).
    Text,
    /// Image, carrying specific format information.
    Image(ImageFormat),
    /// PDF document.
    Pdf,
    /// Audio data.
    Audio(AudioFormat),
    /// Video data.
    Video(VideoFormat),
    /// Unknown or unrecognized binary format.
    Binary,
}

/// Image format sub-type.
/// Unknown image formats are not represented by `Other` but fall back to `MediaType::Binary`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ImageFormat {
    /// PNG image.
    Png,
    /// JPEG image.
    Jpeg,
    /// GIF image.
    Gif,
    /// WebP image.
    Webp,
    /// SVG image.
    Svg,
    /// BMP image.
    Bmp,
}

/// Audio format sub-type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AudioFormat {
    /// MP3 audio.
    Mp3,
    /// WAV audio.
    Wav,
    /// Ogg Vorbis audio.
    Ogg,
    /// AAC audio.
    Aac,
    /// FLAC audio.
    Flac,
}

/// Video format sub-type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum VideoFormat {
    /// MP4 video.
    Mp4,
    /// WebM video.
    Webm,
    /// AVI video.
    Avi,
    /// QuickTime/MOV video.
    Mov,
}

fn eq_any_ignore_ascii_case(s: &str, candidates: &[&str]) -> bool {
    candidates.iter().any(|c| s.eq_ignore_ascii_case(c))
}

impl MediaType {
    /// Infers media type from a file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        let ext = ext.trim_start_matches('.');
        if eq_any_ignore_ascii_case(
            ext,
            &[
                "txt", "md", "json", "csv", "yaml", "yml", "xml", "html", "htm", "rs", "py", "js",
                "ts", "go", "java", "c", "cpp", "h", "hpp", "toml", "ini", "cfg", "log",
            ],
        ) {
            Some(MediaType::Text)
        } else if ext.eq_ignore_ascii_case("png") {
            Some(MediaType::Image(ImageFormat::Png))
        } else if ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg") {
            Some(MediaType::Image(ImageFormat::Jpeg))
        } else if ext.eq_ignore_ascii_case("gif") {
            Some(MediaType::Image(ImageFormat::Gif))
        } else if ext.eq_ignore_ascii_case("webp") {
            Some(MediaType::Image(ImageFormat::Webp))
        } else if ext.eq_ignore_ascii_case("svg") {
            Some(MediaType::Image(ImageFormat::Svg))
        } else if ext.eq_ignore_ascii_case("bmp") {
            Some(MediaType::Image(ImageFormat::Bmp))
        } else if ext.eq_ignore_ascii_case("pdf") {
            Some(MediaType::Pdf)
        } else if ext.eq_ignore_ascii_case("mp3") {
            Some(MediaType::Audio(AudioFormat::Mp3))
        } else if ext.eq_ignore_ascii_case("wav") {
            Some(MediaType::Audio(AudioFormat::Wav))
        } else if ext.eq_ignore_ascii_case("ogg") {
            Some(MediaType::Audio(AudioFormat::Ogg))
        } else if ext.eq_ignore_ascii_case("aac") {
            Some(MediaType::Audio(AudioFormat::Aac))
        } else if ext.eq_ignore_ascii_case("flac") {
            Some(MediaType::Audio(AudioFormat::Flac))
        } else if ext.eq_ignore_ascii_case("mp4") {
            Some(MediaType::Video(VideoFormat::Mp4))
        } else if ext.eq_ignore_ascii_case("webm") {
            Some(MediaType::Video(VideoFormat::Webm))
        } else if ext.eq_ignore_ascii_case("avi") {
            Some(MediaType::Video(VideoFormat::Avi))
        } else if ext.eq_ignore_ascii_case("mov") {
            Some(MediaType::Video(VideoFormat::Mov))
        } else {
            None
        }
    }

    /// Returns the default MIME type for this media type.
    pub fn mime_type(&self) -> Option<&'static str> {
        match self {
            MediaType::Text => Some("text/plain"),
            MediaType::Image(fmt) => Some(fmt.mime_type()),
            MediaType::Pdf => Some("application/pdf"),
            MediaType::Audio(fmt) => Some(fmt.mime_type()),
            MediaType::Video(fmt) => Some(fmt.mime_type()),
            MediaType::Binary => None,
        }
    }
}

impl ImageFormat {
    /// Returns the MIME type for this image format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Gif => "image/gif",
            ImageFormat::Webp => "image/webp",
            ImageFormat::Svg => "image/svg+xml",
            ImageFormat::Bmp => "image/bmp",
        }
    }
}

impl AudioFormat {
    /// Returns the MIME type for this audio format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "audio/mpeg",
            AudioFormat::Wav => "audio/wav",
            AudioFormat::Ogg => "audio/ogg",
            AudioFormat::Aac => "audio/aac",
            AudioFormat::Flac => "audio/flac",
        }
    }
}

impl VideoFormat {
    /// Returns the MIME type for this video format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            VideoFormat::Mp4 => "video/mp4",
            VideoFormat::Webm => "video/webm",
            VideoFormat::Avi => "video/x-msvideo",
            VideoFormat::Mov => "video/quicktime",
        }
    }
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MediaType::Text => write!(f, "text"),
            MediaType::Image(fmt) => write!(f, "image/{}", fmt),
            MediaType::Pdf => write!(f, "pdf"),
            MediaType::Audio(fmt) => write!(f, "audio/{}", fmt),
            MediaType::Video(fmt) => write!(f, "video/{}", fmt),
            MediaType::Binary => write!(f, "binary"),
        }
    }
}

impl std::fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageFormat::Png => write!(f, "png"),
            ImageFormat::Jpeg => write!(f, "jpeg"),
            ImageFormat::Gif => write!(f, "gif"),
            ImageFormat::Webp => write!(f, "webp"),
            ImageFormat::Svg => write!(f, "svg"),
            ImageFormat::Bmp => write!(f, "bmp"),
        }
    }
}

impl std::fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioFormat::Mp3 => write!(f, "mp3"),
            AudioFormat::Wav => write!(f, "wav"),
            AudioFormat::Ogg => write!(f, "ogg"),
            AudioFormat::Aac => write!(f, "aac"),
            AudioFormat::Flac => write!(f, "flac"),
        }
    }
}

impl std::fmt::Display for VideoFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VideoFormat::Mp4 => write!(f, "mp4"),
            VideoFormat::Webm => write!(f, "webm"),
            VideoFormat::Avi => write!(f, "avi"),
            VideoFormat::Mov => write!(f, "mov"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_extension_text() {
        assert_eq!(MediaType::from_extension("txt"), Some(MediaType::Text));
        assert_eq!(MediaType::from_extension("md"), Some(MediaType::Text));
        assert_eq!(MediaType::from_extension("json"), Some(MediaType::Text));
        assert_eq!(MediaType::from_extension(".rs"), Some(MediaType::Text));
    }

    #[test]
    fn test_from_extension_image() {
        assert_eq!(
            MediaType::from_extension("png"),
            Some(MediaType::Image(ImageFormat::Png))
        );
        assert_eq!(
            MediaType::from_extension("jpg"),
            Some(MediaType::Image(ImageFormat::Jpeg))
        );
        assert_eq!(
            MediaType::from_extension("webp"),
            Some(MediaType::Image(ImageFormat::Webp))
        );
    }

    #[test]
    fn test_from_extension_pdf() {
        assert_eq!(MediaType::from_extension("pdf"), Some(MediaType::Pdf));
    }

    #[test]
    fn test_from_extension_audio() {
        assert_eq!(
            MediaType::from_extension("mp3"),
            Some(MediaType::Audio(AudioFormat::Mp3))
        );
        assert_eq!(
            MediaType::from_extension("wav"),
            Some(MediaType::Audio(AudioFormat::Wav))
        );
    }

    #[test]
    fn test_from_extension_video() {
        assert_eq!(
            MediaType::from_extension("mp4"),
            Some(MediaType::Video(VideoFormat::Mp4))
        );
        assert_eq!(
            MediaType::from_extension("mov"),
            Some(MediaType::Video(VideoFormat::Mov))
        );
    }

    #[test]
    fn test_from_extension_unknown() {
        assert_eq!(MediaType::from_extension("xyz"), None);
    }

    #[test]
    fn test_mime_type() {
        assert_eq!(MediaType::Text.mime_type(), Some("text/plain"));
        assert_eq!(
            MediaType::Image(ImageFormat::Png).mime_type(),
            Some("image/png")
        );
        assert_eq!(MediaType::Pdf.mime_type(), Some("application/pdf"));
        assert_eq!(
            MediaType::Audio(AudioFormat::Mp3).mime_type(),
            Some("audio/mpeg")
        );
        assert_eq!(
            MediaType::Video(VideoFormat::Mp4).mime_type(),
            Some("video/mp4")
        );
        assert_eq!(MediaType::Binary.mime_type(), None);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", MediaType::Text), "text");
        assert_eq!(
            format!("{}", MediaType::Image(ImageFormat::Png)),
            "image/png"
        );
    }
}
