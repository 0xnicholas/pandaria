#![deny(missing_docs)]
//! File handling utilities for Pandaria multimodal inputs.
//!
//! `pawbun-files` provides a unified, type-safe, and extensible layer for
//! working with files of different media types (text, image, PDF, audio,
//! video) across local paths, URLs, and in-memory bytes.
//!
//! # Quick Start
//!
//! ```no_run
//! use pawbun_files::{File, DefaultFileLoader, FileLoader, OpenAiFormat, ProviderFormat};
//!
//! let loader = DefaultFileLoader::new();
//! let file = File::from_path("./chart.png").with_key("sales_chart");
//!
//! let loaded = loader.load(&file).expect("load file");
//!
//! let formatter = OpenAiFormat;
//! let block = formatter.format_content(&loaded.content).expect("format");
//! ```
//!
//! # Architecture
//!
//! The crate is organized into four layers:
//!
//! | Layer | Types | Responsibility |
//! |---|---|---|
//! | **Type** | [`File`], [`MediaType`], [`MediaContent`] | Unified representation |
//! | **Source** | [`FileSource`] | Abstract file origins (local / URL / bytes) |
//! | **Loader** | [`FileLoader`], [`AsyncFileLoader`], [`DefaultFileLoader`] | Read, validate, parse |
//! | **Provider** | [`ProviderFormat`], [`OpenAiFormat`], [`AnthropicFormat`], [`GeminiFormat`], [`AzureOpenAiFormat`] | Format for LLM APIs |
//!
//! # Feature Flags
//!
//! | Feature | Description | Extra Dependencies |
//! |---|---|---|
//! | `url-source` | Enables HTTP download for [`FileSource::Url`] | `reqwest` |
//! | `image-meta` | Enables image dimension extraction | `image` |
//! | `tracing` | Adds `tracing` spans to loading and formatting | `tracing` |
//! | `full` | Enables all features | — |
//!
//! # Examples
//!
//! ## Constructing a file
//!
//! ```
//! use pawbun_files::{File, MediaType, ImageFormat};
//!
//! // From a local path (media type auto-detected from extension)
//! let file = File::from_path("./report.pdf");
//!
//! // From a URL
//! let file = File::from_url("https://example.com/chart.png");
//!
//! // From in-memory bytes
//! use bytes::Bytes;
//! let data = Bytes::from_static(b"hello world");
//! let file = File::from_bytes(data, "note.txt");
//! ```
//!
//! ## Loading with a sandbox
//!
//! ```no_run
//! use pawbun_files::{File, DefaultFileLoader, FileLoader};
//!
//! let loader = DefaultFileLoader::with_base_dir("/app/data");
//! let file = File::from_path("./report.txt");
//! let loaded = loader.load(&file).expect("load file");
//! ```
//!
//! ## Async loading
//!
//! ```no_run
//! use pawbun_files::{File, DefaultFileLoader, AsyncFileLoader};
//!
//! # async fn example() {
//! let loader = DefaultFileLoader::new();
//! let file = File::from_path("./chart.png");
//! let loaded = loader.load_async(&file).await.expect("load");
//! # }
//! ```
//!
//! ## Switching providers
//!
//! ```no_run
//! use pawbun_files::{File, DefaultFileLoader, FileLoader, GeminiFormat, ProviderFormat};
//!
//! let loader = DefaultFileLoader::new();
//! let file = File::from_path("./diagram.png");
//! let loaded = loader.load(&file).unwrap();
//!
//! let gemini = GeminiFormat;
//! let block = gemini.format_content(&loaded.content).unwrap();
//! ```
//!
//! ## Constraints
//!
//! ```no_run
//! use pawbun_files::{File, FileConstraints, OverflowMode, MediaType, ImageFormat};
//!
//! let file = File::from_path("./image.png")
//!     .with_constraints(FileConstraints {
//!         max_size_bytes: Some(5 * 1024 * 1024),
//!         allowed_media_types: Some(vec![MediaType::Image(ImageFormat::Png)]),
//!         overflow_mode: OverflowMode::Strict,
//!         ..Default::default()
//!     });
//! ```

/// File constraints (size, type, overflow mode).
pub mod constraints;
/// Media content types (text, image, audio, video, PDF, binary).
pub mod content;
/// File representation and sources.
pub mod file;
/// File loader traits and default implementation.
pub mod loader;
/// Media type definitions and detection.
pub mod media;
/// Provider formatting for LLM APIs.
pub mod provider;

// Re-export core types for ergonomic usage.
pub use constraints::{AutoStrategy, ConstraintError, FileConstraints, OverflowMode};
pub use content::{
    AudioContent, BinaryContent, ImageContent, MediaContent, PdfContent, TextContent, VideoContent,
};
pub use file::{File, FileMetadata, FileSource};
pub use loader::{AsyncFileLoader, DefaultFileLoader, FileLoader, LoadError, LoadedContent};
pub use media::{AudioFormat, ImageFormat, MediaType, VideoFormat};
pub use provider::{
    AnthropicFormat, AzureOpenAiFormat, FormatError, GeminiFormat, OpenAiFormat,
    ProviderConstraints, ProviderFormat, TransmissionMethod,
};
