use std::borrow::Cow;
use std::path::PathBuf;

use serde_json::json;

use crate::{Tool, ToolError, ToolParameter, ToolResult};

/// 列出目录内容的工具。
///
/// 输入为 JSON 字符串，包含 `path` 字段。返回该路径下的文件和子目录列表，
/// 每项包含名称、类型（`file` 或 `directory`）和大小（文件）。
///
/// 支持通过 `base_dir` 限制可访问的文件范围，防止路径遍历攻击。
///
/// # Example
/// ```
/// use pawbun_toolkit::{Tool, DirectoryListTool};
///
/// let tool = DirectoryListTool::default();
/// assert_eq!(tool.name(), "directory_list");
/// ```
#[derive(Debug, Default)]
pub struct DirectoryListTool {
    /// 基础目录，所有相对路径均在此目录下解析。
    /// `None` 表示使用当前工作目录。
    pub base_dir: Option<PathBuf>,
}

impl DirectoryListTool {
    /// 创建一个新的 `DirectoryListTool`，可指定基础目录。
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base_dir.into()),
        }
    }

    /// 解析并验证目标路径，防止路径遍历。
    fn resolve_path(&self, input: &str) -> Result<PathBuf, ToolError> {
        crate::tools::path_utils::resolve_sandbox_path(self.base_dir.as_deref(), input)
    }
}

impl Tool for DirectoryListTool {
    fn name(&self) -> &str {
        "directory_list"
    }

    fn description(&self) -> &str {
        "List files and directories at a given path. Input should be JSON with a 'path' field."
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![ToolParameter {
            name: "path".into(),
            description: "Directory path to list".into(),
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

        let mut entries = Vec::new();
        let dir = std::fs::read_dir(&target)
            .map_err(|e| ToolError::execution_failed(format!("cannot read directory: {e}")))?;

        for entry in dir {
            let entry = entry.map_err(|e| ToolError::execution_failed(e.to_string()))?;
            let metadata = entry
                .metadata()
                .map_err(|e| ToolError::execution_failed(e.to_string()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let entry_type = if metadata.is_dir() {
                "directory"
            } else if metadata.is_file() {
                "file"
            } else if metadata.is_symlink() {
                "symlink"
            } else {
                "other"
            };

            let mut item = json!({
                "name": name,
                "type": entry_type,
            });

            if metadata.is_file() {
                item["size"] = json!(metadata.len());
            }

            entries.push(item);
        }

        // 按目录优先、然后按名称排序
        entries.sort_by(|a, b| {
            let ta = a["type"].as_str().unwrap_or("");
            let tb = b["type"].as_str().unwrap_or("");
            let na = a["name"].as_str().unwrap_or("");
            let nb = b["name"].as_str().unwrap_or("");
            ta.cmp(tb).reverse().then_with(|| na.cmp(nb))
        });

        Ok(ToolResult {
            success: true,
            content: serde_json::to_string_pretty(&entries)
                .map_err(|e| ToolError::serialization(e.to_string()))?,
            metadata: Some(json!({"path": target.to_string_lossy(), "count": entries.len()})),
            elapsed_ms: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn setup(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("pawbun_toolkit_dir_list_test")
            .join(test_name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_list_directory() {
        let dir = setup("list_dir");
        std::fs::File::create(dir.join("a.txt")).unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();

        let tool = DirectoryListTool::new(&dir);
        let result = tool.execute(r#"{"path": "."}"#).unwrap();

        assert!(result.success);
        let items: Vec<serde_json::Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(items.len(), 2);

        let names: Vec<&str> = items.iter().map(|v| v["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"a.txt"));
        assert!(names.contains(&"sub"));
    }

    #[test]
    fn test_path_traversal_blocked() {
        let dir = setup("traversal");
        let tool = DirectoryListTool::new(&dir);
        let result = tool.execute(r#"{"path": "../secret"}"#);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("path traversal") || err.contains("invalid path"));
    }

    #[test]
    fn test_missing_path_field() {
        let dir = setup("missing_path");
        let tool = DirectoryListTool::new(&dir);
        let result = tool.execute(r#"{}"#);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing 'path' field"));
    }

    #[test]
    fn test_file_size_present() {
        let dir = setup("file_size");
        {
            let mut f = std::fs::File::create(dir.join("data.bin")).unwrap();
            f.write_all(b"hello").unwrap();
        }

        let tool = DirectoryListTool::new(&dir);
        let result = tool.execute(r#"{"path": "."}"#).unwrap();

        let items: Vec<serde_json::Value> = serde_json::from_str(&result.content).unwrap();
        let file_item = items.iter().find(|v| v["name"] == "data.bin").unwrap();
        assert_eq!(file_item["type"], "file");
        assert_eq!(file_item["size"], 5);
    }
}
