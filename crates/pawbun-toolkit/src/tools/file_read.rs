use std::borrow::Cow;
use std::path::PathBuf;

use serde_json::json;

use crate::{Tool, ToolError, ToolParameter, ToolResult};

/// 读取文件内容的工具。
///
/// 支持通过 `base_dir` 限制可访问的文件范围，防止路径遍历攻击。
/// 输入为 JSON 字符串，包含 `path` 字段。
///
/// # Example
/// ```
/// use pawbun_toolkit::{Tool, FileReadTool};
///
/// let tool = FileReadTool::default();
/// assert_eq!(tool.name(), "file_read");
/// ```
#[derive(Debug, Default)]
pub struct FileReadTool {
    /// 基础目录，所有相对路径均在此目录下解析。
    /// `None` 表示使用当前工作目录。
    pub base_dir: Option<PathBuf>,
    /// 最大读取文件大小（字节）。`None` 表示不限制。
    pub max_size_bytes: Option<usize>,
}

impl FileReadTool {
    /// 创建一个新的 `FileReadTool`，可指定基础目录。
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base_dir.into()),
            max_size_bytes: None,
        }
    }

    /// 设置最大读取文件大小（字节）。
    pub fn with_max_size(mut self, max_size: usize) -> Self {
        self.max_size_bytes = Some(max_size);
        self
    }

    /// 解析并验证目标路径，防止路径遍历。
    fn resolve_path(&self, input: &str) -> Result<PathBuf, ToolError> {
        crate::tools::path_utils::resolve_sandbox_path(self.base_dir.as_deref(), input)
    }
}

impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Input should be JSON with a 'path' field."
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![ToolParameter {
            name: "path".into(),
            description: "Relative or absolute file path".into(),
            required: true,
            schema: json!({"type": "string"}),
        }])
    }

    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value = crate::json_utils::parse(input)?;

        let path = parsed
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'path' field"))?;

        let target = self.resolve_path(path)?;

        // Check file size before reading, if a limit is configured.
        if let Some(max_size) = self.max_size_bytes {
            let metadata = std::fs::metadata(&target)
                .map_err(|e| ToolError::execution_failed(e.to_string()))?;
            if metadata.len() > max_size as u64 {
                return Err(ToolError::execution_failed(format!(
                    "file size {} exceeds maximum of {}",
                    metadata.len(),
                    max_size
                )));
            }
        }

        let content = std::fs::read_to_string(&target)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;

        Ok(ToolResult {
            success: true,
            content,
            metadata: Some(json!({"path": target.to_string_lossy()})),
            elapsed_ms: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn setup(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("pawbun_toolkit_file_read_test")
            .join(test_name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_read_existing_file() {
        let dir = setup("read_existing");
        let file_path = dir.join("test.txt");
        {
            let mut f = std::fs::File::create(&file_path).unwrap();
            f.write_all(b"hello world").unwrap();
        }

        let tool = FileReadTool::new(&dir);
        let result = tool.execute(r#"{"path": "test.txt"}"#).unwrap();

        assert!(result.success);
        assert_eq!(result.content, "hello world");
    }

    #[test]
    fn test_path_traversal_blocked() {
        let dir = setup("traversal");
        let tool = FileReadTool::new(&dir);
        let result = tool.execute(r#"{"path": "../secret.txt"}"#);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("path traversal") || err.contains("invalid path"));
    }

    #[test]
    fn test_missing_file() {
        let dir = setup("missing");
        let tool = FileReadTool::new(&dir);
        let result = tool.execute(r#"{"path": "nonexistent.txt"}"#);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid path"));
    }

    #[test]
    fn test_invalid_json_input() {
        let dir = setup("invalid_json");
        let tool = FileReadTool::new(&dir);
        let result = tool.execute("not json");

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("serialization"));
    }

    #[test]
    fn test_missing_path_field() {
        let dir = setup("missing_path");
        let tool = FileReadTool::new(&dir);
        let result = tool.execute(r#"{"foo": "bar"}"#);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing 'path' field"));
    }

    #[test]
    fn test_default_base_dir() {
        let tool = FileReadTool::default();
        assert!(tool.base_dir.is_none());
        assert_eq!(tool.name(), "file_read");
    }
}
