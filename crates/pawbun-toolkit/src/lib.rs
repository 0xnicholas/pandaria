#![deny(missing_docs)]
//! Agent tools for the Pandaria ecosystem.
//!
//! `pawbun-toolkit` provides the core [`Tool`] trait and [`ToolKit`] registry,
//! enabling agents to discover and invoke capabilities in a structured way.
//!
//! # Quick Start
//!
//! ```no_run
//! use pawbun_toolkit::{ToolKit, ToolExecutor, FileReadTool};
//!
//! let mut toolkit = ToolKit::new();
//! toolkit.register(Box::new(FileReadTool::default()));
//!
//! let result = toolkit.execute("file_read", r#"{"path": "README.md"}"#).unwrap();
//! println!("{}", result.content);
//! ```
//!
//! # Architecture
//!
//! The crate is organized around a few core abstractions:
//!
//! - [`Tool`] — The fundamental trait that every tool implements.
//! - [`ToolKit`] — A registry that holds tools and executes them by name.
//! - [`ToolRegistry`] — Trait for discovering what tools are available.
//! - [`ToolExecutor`] — Trait for invoking a tool by name.
//! - [`AsyncToolExecutor`] — Trait for async tool invocation.
//! - [`AsyncTool`] — Trait for tools with async execution.
//! - [`BlockingExecutor`] — Pluggable blocking execution strategy for async contexts.
//! - [`ToolResult`] — Uniform return type for all tool executions.
//! - [`ToolError`] — Error type covering invalid input, execution failures,
//!   missing tools, timeouts, and serialization issues.

/// Async tool execution abstractions.
pub mod async_tool;
/// Error types for tool operations.
pub mod error;
/// MCP (Model Context Protocol) client adapters.
pub mod mcp;
/// Tool registry traits for discovery and execution.
pub mod registry;
/// Core [`Tool`] trait definition.
pub mod tool;
/// [`ToolKit`] registry implementation.
pub mod toolkit;
/// Built-in tool implementations.
pub mod tools;
/// Shared types: [`ToolParameter`], [`ToolResult`].
pub mod types;

mod json_utils;

pub use async_tool::{AsyncTool, BlockingExecutor};
pub use error::ToolError;
pub use registry::{AsyncToolExecutor, ToolExecutor, ToolRegistry};
pub use tool::Tool;
pub use toolkit::ToolKit;
pub use tools::{CodeExecuteTool, DirectoryListTool, FileReadTool, FileWriteTool, VisionTool};
pub use types::{ToolParameter, ToolResult};

#[cfg(feature = "csv")]
pub use tools::CsvQueryTool;
#[cfg(feature = "jsonpath")]
pub use tools::JsonQueryTool;
#[cfg(feature = "http")]
pub use tools::{WebFetchTool, WebSearchTool};

#[cfg(feature = "tokio")]
pub use async_tool::TokioExecutor;

#[cfg(feature = "tokio")]
pub use tools::LocalCodeExecutor;

#[cfg(feature = "macros")]
pub use pawbun_toolkit_macros::pawbun_tool;
