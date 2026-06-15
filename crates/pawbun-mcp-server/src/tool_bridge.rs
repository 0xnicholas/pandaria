use std::borrow::Cow;

use pawbun_files::{DefaultFileLoader, File, FileLoader};
use pawbun_toolkit::{Tool, ToolError, ToolKit, ToolParameter, ToolRegistry, ToolResult};
use serde_json::json;

/// Register bridge tools derived from the FileLoader.
///
/// Automatically adds `file_read` and `file_list` tools if not already
/// present in the toolkit (user-registered tools take priority).
pub(crate) fn register_bridge_tools(toolkit: &mut ToolKit, loader: DefaultFileLoader) {
    // Check before registering — user tools take priority
    if toolkit.get("file_read").is_none() {
        toolkit.register(Box::new(FileReadBridgeTool {
            loader: loader.clone(),
        }));
    }
    if toolkit.get("file_list").is_none() {
        toolkit.register(Box::new(FileListBridgeTool {
            loader: loader.clone(),
        }));
    }
}

// ── FileReadBridgeTool ──

#[derive(Debug)]
struct FileReadBridgeTool {
    loader: DefaultFileLoader,
}

impl Tool for FileReadBridgeTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read contents of a file. Supports text, images, PDFs, audio, and video."
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![ToolParameter {
            name: "path".into(),
            description: "Relative or absolute file path to read".into(),
            required: true,
            schema: json!({"type": "string"}),
        }])
    }

    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value =
            serde_json::from_str(input).map_err(|e| ToolError::serialization(e.to_string()))?;

        let path = parsed
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'path' field"))?;

        let file = File::from_path(path);
        let loaded = self
            .loader
            .load(&file)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;

        let content_json = serde_json::to_string(&loaded.content)
            .map_err(|e| ToolError::serialization(e.to_string()))?;

        Ok(ToolResult {
            success: true,
            content: content_json,
            metadata: Some(json!({"path": path, "media_type": loaded.content.media_type().to_string()})),
            elapsed_ms: None,
        })
    }
}

// ── FileListBridgeTool ──

#[derive(Debug)]
struct FileListBridgeTool {
    loader: DefaultFileLoader,
}

impl Tool for FileListBridgeTool {
    fn name(&self) -> &str {
        "file_list"
    }

    fn description(&self) -> &str {
        "List file metadata at a given path."
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![ToolParameter {
            name: "path".into(),
            description: "File path to get metadata for".into(),
            required: true,
            schema: json!({"type": "string"}),
        }])
    }

    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value =
            serde_json::from_str(input).map_err(|e| ToolError::serialization(e.to_string()))?;

        let path = parsed
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'path' field"))?;

        let file = File::from_path(path);
        let meta = self
            .loader
            .metadata(&file)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;

        let meta_json = json!({
            "name": meta.name,
            "mime_type": meta.mime_type,
            "size_bytes": meta.size_bytes,
        });

        Ok(ToolResult {
            success: true,
            content: meta_json.to_string(),
            metadata: Some(json!({"path": path})),
            elapsed_ms: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawbun_toolkit::ToolExecutor;
    use std::io::Write;

    fn setup_tmp_dir(test_name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("pawbun_mcp_server_bridge_test")
            .join(test_name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_file_read_bridge_tool() {
        let dir = setup_tmp_dir("file_read");
        let path = dir.join("hello.txt");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"bridge test content").unwrap();
        }

        let loader = DefaultFileLoader::with_base_dir(&dir);
        let tool = FileReadBridgeTool { loader };

        let input = json!({"path": "hello.txt"}).to_string();
        let result = tool.execute(&input).unwrap();
        assert!(result.success);
        assert!(result.content.contains("bridge test content"));
    }

    #[test]
    fn test_file_read_path_traversal_blocked() {
        let dir = setup_tmp_dir("traversal");
        let loader = DefaultFileLoader::with_base_dir(&dir);
        let tool = FileReadBridgeTool { loader };

        let input = json!({"path": "../secret.txt"}).to_string();
        let result = tool.execute(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_file_list_bridge_tool() {
        let dir = setup_tmp_dir("file_list");
        std::fs::write(dir.join("a.txt"), "hello").unwrap();

        let loader = DefaultFileLoader::with_base_dir(&dir);
        let tool = FileListBridgeTool { loader };

        let input = json!({"path": "a.txt"}).to_string();
        let result = tool.execute(&input).unwrap();
        assert!(result.success);
        assert!(result.content.contains("a.txt"));
        assert!(result.content.contains("5")); // size_bytes
    }

    #[test]
    fn test_deduplication_user_tool_priority() {
        use pawbun_toolkit::{Tool, ToolError, ToolResult};
        use std::borrow::Cow;

        #[derive(Debug)]
        struct CustomFileRead;

        impl Tool for CustomFileRead {
            fn name(&self) -> &str {
                "file_read"
            }
            fn description(&self) -> &str {
                "custom"
            }
            fn parameters(&self) -> Cow<'static, [ToolParameter]> {
                Cow::Owned(vec![])
            }
            fn execute(&self, _: &str) -> Result<ToolResult, ToolError> {
                Ok(ToolResult {
                    success: true,
                    content: "custom_output".into(),
                    metadata: None,
                    elapsed_ms: None,
                })
            }
        }

        let mut toolkit = ToolKit::new();
        toolkit.register(Box::new(CustomFileRead));

        let loader = DefaultFileLoader::new();
        register_bridge_tools(&mut toolkit, loader);

        // Custom tool should still be there, not overwritten
        let result = toolkit.execute("file_read", "{}").unwrap();
        assert_eq!(result.content, "custom_output");
    }
}
