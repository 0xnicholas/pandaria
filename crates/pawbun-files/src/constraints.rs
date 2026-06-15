//! File constraints and validation.
//!
//! Alpha stage: size limits and media type whitelist are enforced.
//! Full dimension/duration checks and `Auto` downgrading will be enabled in Beta.

use serde::{Deserialize, Serialize};

use crate::content::MediaContent;
use crate::media::MediaType;

/// File constraint conditions.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FileConstraints {
    /// Maximum file size in bytes. `None` means no limit.
    pub max_size_bytes: Option<u64>,
    /// Allowed media type whitelist. `None` means all allowed.
    pub allowed_media_types: Option<Vec<MediaType>>,
    /// Maximum image width in pixels (reserved for Beta).
    pub max_image_width: Option<u32>,
    /// Maximum image height in pixels (reserved for Beta).
    pub max_image_height: Option<u32>,
    /// Maximum audio duration in seconds (reserved for Beta).
    pub max_audio_duration: Option<f64>,
    /// Maximum video duration in seconds (reserved for Beta).
    pub max_video_duration: Option<f64>,
    /// Overflow handling mode.
    pub overflow_mode: OverflowMode,
    /// Auto-downgrade strategy config (only effective when `overflow_mode = Auto`; reserved for Beta).
    pub auto_strategy: AutoStrategy,
}

/// Handling strategy when file content exceeds constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OverflowMode {
    /// Fail directly.
    #[default]
    Strict,
    /// Log warning but continue processing.
    Warn,
    /// Auto-downgrade processing.
    ///
    /// ⚠️ **Unimplemented in Alpha**: `Auto` currently behaves identically to `Strict`.
    /// Actual downgrading (image resize, audio truncate, etc.) will be enabled in Beta.
    Auto,
}

/// Auto-downgrade strategy configuration.
///
/// ⚠️ **Unimplemented in Alpha**: the fields below are reserved for Beta.
/// Setting them has no effect until auto-downgrading is implemented.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoStrategy {
    /// Image resize: target max width (keep aspect ratio).
    pub image_target_width: Option<u32>,
    /// Image resize: target max height (keep aspect ratio).
    pub image_target_height: Option<u32>,
    /// Image compression quality (0-100, JPEG/WebP only).
    pub image_quality: u8,
    /// Audio truncate: keep first N seconds.
    pub audio_keep_first_seconds: Option<f64>,
    /// PDF chunking: max pages per chunk.
    pub pdf_max_pages_per_chunk: Option<usize>,
    /// Video fallback: extract frame at N seconds as image substitute.
    pub video_extract_frame_at: Option<f64>,
}

impl Default for AutoStrategy {
    fn default() -> Self {
        Self {
            image_target_width: Some(2048),
            image_target_height: Some(2048),
            image_quality: 85,
            audio_keep_first_seconds: None,
            pdf_max_pages_per_chunk: None,
            video_extract_frame_at: Some(0.0),
        }
    }
}

/// Constraint validation error.
#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum ConstraintError {
    /// File size exceeds configured limit.
    #[error("size {actual} exceeds limit {limit}")]
    SizeExceeded {
        /// Actual file size in bytes.
        actual: u64,
        /// Configured size limit in bytes.
        limit: u64,
    },
    /// Media type not in allowed whitelist.
    #[error("media type {0} not in allowed list")]
    TypeNotAllowed(MediaType),
    /// Image dimensions exceed configured limits.
    #[error("image dimensions {width}x{height} exceed limit")]
    ImageDimensionsExceeded {
        /// Actual image width in pixels.
        width: u32,
        /// Actual image height in pixels.
        height: u32,
    },
    /// Audio/video duration exceeds configured limit.
    #[error("duration {actual}s exceeds limit {limit}s")]
    DurationExceeded {
        /// Actual duration in seconds.
        actual: f64,
        /// Configured duration limit in seconds.
        limit: f64,
    },
}

/// Downgrades an image according to the given `AutoStrategy`.
///
/// Requires the `image-meta` feature. Resizes the image to fit within
/// `image_target_width` and `image_target_height` while preserving aspect ratio.
/// If the image is already smaller than the target, it is left unchanged.
///
/// **Note**: Quality adjustment (JPEG/WebP) is not yet implemented.
#[cfg(feature = "image-meta")]
pub fn downgrade_image(
    img: &mut crate::content::ImageContent,
    strategy: &AutoStrategy,
) -> Result<(), String> {
    use image::imageops::FilterType;

    let (target_w, target_h) = match (strategy.image_target_width, strategy.image_target_height) {
        (Some(w), Some(h)) => (w, h),
        _ => return Ok(()), // no target specified, skip
    };

    let dynamic_img = image::load_from_memory(&img.bytes).map_err(|e| e.to_string())?;
    let resized = dynamic_img.resize(target_w, target_h, FilterType::Lanczos3);

    // Skip if already smaller than or equal to target.
    if resized.width() >= dynamic_img.width() && resized.height() >= dynamic_img.height() {
        return Ok(());
    }

    let mut buf = Vec::new();
    let format = match img.format {
        crate::media::ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        crate::media::ImageFormat::Png => image::ImageFormat::Png,
        crate::media::ImageFormat::Webp => image::ImageFormat::WebP,
        _ => image::ImageFormat::Png,
    };
    resized
        .write_to(&mut std::io::Cursor::new(&mut buf), format)
        .map_err(|e| e.to_string())?;

    img.bytes = bytes::Bytes::from(buf);
    img.width = Some(resized.width());
    img.height = Some(resized.height());
    Ok(())
}

impl FileConstraints {
    /// Validates whether content satisfies constraints.
    ///
    /// **Missing metadata policy**: if a dimension in `content` is `None`
    /// (e.g. width/height unknown because `image-meta` is not enabled), that dimension
    /// is considered unverifiable and passes silently.
    ///
    /// **Overflow mode**: `Strict` returns an error on the first violation.
    /// `Warn` returns `Ok` but emits a `tracing::warn!` when the `tracing` feature is enabled.
    /// `Auto` is reserved for Beta and currently behaves like `Strict`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(content.media_type = ?content.media_type(), content.size = content.size_bytes())))]
    pub fn check(&self, content: &MediaContent) -> Result<(), ConstraintError> {
        let mut violations = self.collect_violations(content);

        if violations.is_empty() {
            return Ok(());
        }

        match self.overflow_mode {
            OverflowMode::Strict => Err(violations.pop().expect("violations non-empty")),
            OverflowMode::Warn => {
                #[cfg(feature = "tracing")]
                tracing::warn!(violations = ?violations, "file constraints violated (OverflowMode::Warn)");
                Ok(())
            }
            OverflowMode::Auto => {
                #[cfg(feature = "tracing")]
                tracing::warn!(
                    violations = ?violations,
                    "OverflowMode::Auto downgrading not yet implemented in Alpha; treating as Strict"
                );
                Err(violations.pop().expect("violations non-empty"))
            }
        }
    }

    fn collect_violations(&self, content: &MediaContent) -> Vec<ConstraintError> {
        let mut violations = Vec::new();

        if let Some(limit) = self.max_size_bytes {
            let size = content.size_bytes();
            if size > limit {
                violations.push(ConstraintError::SizeExceeded {
                    actual: size,
                    limit,
                });
            }
        }

        if let Some(ref allowed) = self.allowed_media_types {
            if !allowed.contains(&content.media_type()) {
                violations.push(ConstraintError::TypeNotAllowed(content.media_type()));
            }
        }

        if let MediaContent::Image(img) = content {
            if let (Some(max_w), Some(w)) = (self.max_image_width, img.width) {
                if w > max_w {
                    violations.push(ConstraintError::ImageDimensionsExceeded {
                        width: w,
                        height: img.height.unwrap_or(0),
                    });
                }
            }
            if let (Some(max_h), Some(h)) = (self.max_image_height, img.height) {
                if h > max_h {
                    violations.push(ConstraintError::ImageDimensionsExceeded {
                        width: img.width.unwrap_or(0),
                        height: h,
                    });
                }
            }
        }

        if let MediaContent::Audio(audio) = content {
            if let Some(limit) = self.max_audio_duration {
                if let Some(duration) = audio.duration {
                    if duration > limit {
                        violations.push(ConstraintError::DurationExceeded {
                            actual: duration,
                            limit,
                        });
                    }
                }
            }
        }

        if let MediaContent::Video(video) = content {
            if let Some(limit) = self.max_video_duration {
                if let Some(duration) = video.duration {
                    if duration > limit {
                        violations.push(ConstraintError::DurationExceeded {
                            actual: duration,
                            limit,
                        });
                    }
                }
            }
        }

        violations
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{BinaryContent, ImageContent, TextContent};
    use crate::media::ImageFormat;
    use bytes::Bytes;

    #[test]
    fn test_check_no_constraints() {
        let c = FileConstraints::default();
        let content = MediaContent::Text(TextContent {
            text: "hello".into(),
            encoding: None,
        });
        assert!(c.check(&content).is_ok());
    }

    #[test]
    fn test_check_size_pass() {
        let c = FileConstraints {
            max_size_bytes: Some(10),
            ..Default::default()
        };
        let content = MediaContent::Text(TextContent {
            text: "hello".into(),
            encoding: None,
        });
        assert!(c.check(&content).is_ok());
    }

    #[test]
    fn test_check_size_fail() {
        let c = FileConstraints {
            max_size_bytes: Some(3),
            ..Default::default()
        };
        let content = MediaContent::Text(TextContent {
            text: "hello".into(),
            encoding: None,
        });
        let err = c.check(&content).unwrap_err();
        assert!(matches!(
            err,
            ConstraintError::SizeExceeded {
                actual: 5,
                limit: 3
            }
        ));
    }

    #[test]
    fn test_check_size_exact_boundary() {
        let c = FileConstraints {
            max_size_bytes: Some(5),
            ..Default::default()
        };
        let content = MediaContent::Text(TextContent {
            text: "hello".into(),
            encoding: None,
        });
        assert!(c.check(&content).is_ok());
    }

    #[test]
    fn test_check_type_allowed() {
        let c = FileConstraints {
            allowed_media_types: Some(vec![MediaType::Text, MediaType::Image(ImageFormat::Png)]),
            ..Default::default()
        };
        let content = MediaContent::Image(ImageContent {
            bytes: Bytes::from_static(b"x"),
            format: ImageFormat::Png,
            width: None,
            height: None,
        });
        assert!(c.check(&content).is_ok());
    }

    #[test]
    fn test_check_type_not_allowed() {
        let c = FileConstraints {
            allowed_media_types: Some(vec![MediaType::Text]),
            ..Default::default()
        };
        let content = MediaContent::Binary(BinaryContent {
            bytes: Bytes::from_static(b"x"),
            guessed_type: Some(MediaType::Binary),
        });
        let err = c.check(&content).unwrap_err();
        assert!(matches!(
            err,
            ConstraintError::TypeNotAllowed(MediaType::Binary)
        ));
    }

    #[test]
    fn test_overflow_mode_default() {
        assert_eq!(OverflowMode::default(), OverflowMode::Strict);
    }

    #[test]
    fn test_auto_strategy_default() {
        let s = AutoStrategy::default();
        assert_eq!(s.image_target_width, Some(2048));
        assert_eq!(s.image_target_height, Some(2048));
        assert_eq!(s.image_quality, 85);
        assert_eq!(s.video_extract_frame_at, Some(0.0));
    }

    #[test]
    fn test_overflow_mode_warn_allows_violations() {
        let c = FileConstraints {
            max_size_bytes: Some(3),
            overflow_mode: OverflowMode::Warn,
            ..Default::default()
        };
        let content = MediaContent::Text(TextContent {
            text: "hello".into(),
            encoding: None,
        });
        // In Warn mode, check should return Ok despite size violation.
        assert!(c.check(&content).is_ok());
    }

    #[test]
    fn test_overflow_mode_auto_acts_as_strict() {
        let c = FileConstraints {
            max_size_bytes: Some(3),
            overflow_mode: OverflowMode::Auto,
            ..Default::default()
        };
        let content = MediaContent::Text(TextContent {
            text: "hello".into(),
            encoding: None,
        });
        // Auto is not yet implemented in Alpha; behaves like Strict.
        assert!(c.check(&content).is_err());
    }

    #[cfg(feature = "image-meta")]
    #[test]
    fn test_downgrade_image_resize() {
        use crate::content::ImageContent;
        use crate::media::ImageFormat;
        use bytes::Bytes;

        let img = image::RgbaImage::from_pixel(100, 100, image::Rgba([0, 0, 255, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();

        let mut content = ImageContent {
            bytes: Bytes::from(buf),
            format: ImageFormat::Png,
            width: Some(100),
            height: Some(100),
        };

        let strategy = AutoStrategy {
            image_target_width: Some(50),
            image_target_height: Some(50),
            ..Default::default()
        };

        downgrade_image(&mut content, &strategy).unwrap();
        assert!(content.width.unwrap() <= 50);
        assert!(content.height.unwrap() <= 50);
        assert!(content.bytes.len() < 100 * 100 * 4); // should be smaller
    }

    #[cfg(feature = "image-meta")]
    #[test]
    fn test_downgrade_image_no_op_when_already_small() {
        use crate::content::ImageContent;
        use crate::media::ImageFormat;
        use bytes::Bytes;

        let img = image::RgbaImage::from_pixel(10, 10, image::Rgba([255, 0, 0, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        let original_len = buf.len();

        let mut content = ImageContent {
            bytes: Bytes::from(buf),
            format: ImageFormat::Png,
            width: Some(10),
            height: Some(10),
        };

        let strategy = AutoStrategy {
            image_target_width: Some(100),
            image_target_height: Some(100),
            ..Default::default()
        };

        downgrade_image(&mut content, &strategy).unwrap();
        assert_eq!(content.width, Some(10));
        assert_eq!(content.height, Some(10));
        assert_eq!(content.bytes.len(), original_len);
    }
}
