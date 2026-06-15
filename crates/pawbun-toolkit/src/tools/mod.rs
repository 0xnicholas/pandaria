//! 内置工具集合。
//!
//! 当前提供文件读写、目录列表工具；网络工具在启用 `http` feature 时可用；
//! JSON 查询和 CSV 查询在启用对应 feature 时可用；
//! Vision、Embedding、CodeExecute 为接口占位，需外部集成。

/// Code execution tool interface (placeholder).
pub mod code_execute;
/// Directory listing tool.
pub mod directory_list;
/// File read tool.
pub mod file_read;
/// File write tool.
pub mod file_write;
/// Vision tool interface (placeholder).
pub mod vision;
/// Local subprocess code executor (`tokio` feature).
#[cfg(feature = "tokio")]
pub mod local_code_executor;

/// CSV query tool (`csv` feature).
#[cfg(feature = "csv")]
pub mod csv_query;
/// JSONPath query tool (`jsonpath` feature).
#[cfg(feature = "jsonpath")]
pub mod json_query;
/// Web fetch tool (`http` feature).
#[cfg(feature = "http")]
pub mod web_fetch;
/// Web search tool (`http` feature).
#[cfg(feature = "http")]
pub mod web_search;
#[cfg(feature = "http")]
pub(crate) mod url_utils;

pub(crate) mod path_utils;

pub use code_execute::CodeExecuteTool;
#[cfg(feature = "tokio")]
pub use local_code_executor::LocalCodeExecutor;
pub use directory_list::DirectoryListTool;
pub use file_read::FileReadTool;
pub use file_write::FileWriteTool;
pub use vision::VisionTool;

#[cfg(feature = "csv")]
pub use csv_query::CsvQueryTool;
#[cfg(feature = "jsonpath")]
pub use json_query::JsonQueryTool;
#[cfg(feature = "http")]
pub use web_fetch::WebFetchTool;
#[cfg(feature = "http")]
pub use web_search::WebSearchTool;
