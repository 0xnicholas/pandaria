use crate::types::AgentMessage;

/// Tracks file operations observed in a conversation segment.
#[derive(Debug, Default, Clone)]
pub struct FileOperations {
    pub read: Vec<String>,
    pub written: Vec<String>,
    pub edited: Vec<String>,
}

/// Extracts file operations from assistant tool calls.
pub trait FileOperationExtractor: Send + Sync {
    fn extract(&self, messages: &[AgentMessage]) -> FileOperations;
}

/// Default implementation based on tool name matching.
pub struct DefaultFileOperationExtractor {
    pub read_tool_names: Vec<String>,
    pub write_tool_names: Vec<String>,
    pub edit_tool_names: Vec<String>,
    pub path_arg_name: String,
}

impl Default for DefaultFileOperationExtractor {
    fn default() -> Self {
        Self {
            read_tool_names: vec!["read".to_string()],
            write_tool_names: vec!["write".to_string()],
            edit_tool_names: vec!["edit".to_string()],
            path_arg_name: "path".to_string(),
        }
    }
}

impl FileOperationExtractor for DefaultFileOperationExtractor {
    fn extract(&self, messages: &[AgentMessage]) -> FileOperations {
        let mut ops = FileOperations::default();

        for msg in messages {
            if let AgentMessage::Assistant(assistant) = msg {
                for content in &assistant.content {
                    if let ai_provider::Content::ToolCall(tc) = content {
                        let path = tc
                            .arguments
                            .get(&self.path_arg_name)
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());

                        if let Some(path) = path {
                            if self.read_tool_names.contains(&tc.name) {
                                ops.read.push(path);
                            } else if self.write_tool_names.contains(&tc.name) {
                                ops.written.push(path);
                            } else if self.edit_tool_names.contains(&tc.name) {
                                ops.edited.push(path);
                            }
                        }
                    }
                }
            }
        }

        // Deduplicate and sort
        ops.read.sort_unstable();
        ops.read.dedup();
        ops.written.sort_unstable();
        ops.written.dedup();
        ops.edited.sort_unstable();
        ops.edited.dedup();

        ops
    }
}
