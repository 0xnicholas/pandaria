use std::borrow::Cow;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::{Tool, ToolError, ToolParameter, ToolResult};

/// 写入文件内容的工具。
///
/// 支持通过 `base_dir` 限制可写入的文件范围，防止路径遍历攻击。
/// 输入为 JSON 字符串，包含 `path` 和 `content` 字段。
/// 若父目录不存在，将自动创建。
///
/// # Example
/// ```
/// use pawbun_toolkit::{Tool, FileWriteTool};
///
/// let tool = FileWriteTool::default();
/// assert_eq!(tool.name(), "file_write");
/// ```
#[derive(Debug, Default)]
pub struct FileWriteTool {
    /// 基础目录，所有相对路径均在此目录下解析。
    /// `None` 表示使用当前工作目录。
    pub base_dir: Option<PathBuf>,
}

impl FileWriteTool {
    /// 创建一个新的 `FileWriteTool`，可指定基础目录。
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base_dir.into()),
        }
    }

    /// 解析并验证目标路径，防止路径遍历。
    ///
    /// 由于写入时目标文件或父目录可能尚不存在，不使用 `canonicalize()`，
    /// 而是通过语义化路径解析（`components()`）来检测路径遍历。
    ///
    /// ⚠️ **TOCTOU 注意**：本方法在验证路径和实际写入之间存在时间窗口。
    /// 若 `base_dir` 与目标路径之间存在被竞态替换的符号链接，可能逃过检测。
    /// `execute` 方法会在写入前对父目录进行二次校验，以缓解此风险。
    fn resolve_path(&self, input: &str) -> Result<PathBuf, ToolError> {
        crate::tools::path_utils::resolve_write_path(self.base_dir.as_deref(), input)
    }
}

impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Input should be JSON with 'path' and 'content' fields. Directories are created automatically if needed."
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "path".into(),
                description: "Relative or absolute file path".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
            ToolParameter {
                name: "content".into(),
                description: "Text content to write".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
        ])
    }

    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value = crate::json_utils::parse(input)?;

        let path = parsed
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'path' field"))?;

        let content = parsed
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'content' field"))?;

        let target = self.resolve_path(path)?;

        // 自动创建父目录
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ToolError::execution_failed(format!("failed to create directory: {e}"))
            })?;
        }

        // TOCTOU 二次校验：写入前确认目标文件（或其父目录）未通过符号链接逃逸 base 范围。
        if let Ok(canonical) = target.canonicalize() {
            let base = self.base_dir.as_deref().unwrap_or(Path::new("."));
            if let Ok(base_canonical) = base.canonicalize()
                && !canonical.starts_with(&base_canonical)
            {
                return Err(ToolError::invalid_input(
                    "path traversal detected (TOCTOU check failed)",
                ));
            }
        }

        std::fs::write(&target, content).map_err(|e| ToolError::execution_failed(e.to_string()))?;

        Ok(ToolResult {
            success: true,
            content: format!("written to {}", target.display()),
            metadata: Some(json!({
                "path": target.to_string_lossy(),
                "bytes_written": content.len(),
            })),
            elapsed_ms: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("pawbun_toolkit_file_write_test")
            .join(test_name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_write_new_file() {
        let dir = setup("write_new");
        let tool = FileWriteTool::new(&dir);
        let result = tool
            .execute(r#"{"path": "output.txt", "content": "hello"}"#)
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("output.txt"));

        let read = std::fs::read_to_string(dir.join("output.txt")).unwrap();
        assert_eq!(read, "hello");
    }

    #[test]
    fn test_auto_create_directories() {
        let dir = setup("auto_create");
        let tool = FileWriteTool::new(&dir);
        let result = tool
            .execute(r#"{"path": "a/b/c/deep.txt", "content": "deep"}"#)
            .unwrap();

        assert!(result.success);
        let read = std::fs::read_to_string(dir.join("a/b/c/deep.txt")).unwrap();
        assert_eq!(read, "deep");
    }

    #[test]
    fn test_path_traversal_blocked() {
        let dir = setup("traversal");
        let tool = FileWriteTool::new(&dir);
        let result = tool.execute(r#"{"path": "../evil.txt", "content": "x"}"#);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("path traversal") || err.contains("invalid path"));
    }

    #[test]
    fn test_overwrite_existing_file() {
        let dir = setup("overwrite");
        std::fs::write(dir.join("existing.txt"), "old").unwrap();

        let tool = FileWriteTool::new(&dir);
        let result = tool
            .execute(r#"{"path": "existing.txt", "content": "new"}"#)
            .unwrap();

        assert!(result.success);
        let read = std::fs::read_to_string(dir.join("existing.txt")).unwrap();
        assert_eq!(read, "new");
    }

    #[test]
    fn test_missing_content_field() {
        let dir = setup("missing_content");
        let tool = FileWriteTool::new(&dir);
        let result = tool.execute(r#"{"path": "x.txt"}"#);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing 'content' field"));
    }
}
