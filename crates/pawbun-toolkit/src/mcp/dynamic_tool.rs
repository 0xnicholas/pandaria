//! Dynamic Tool implementation backed by an MCP session.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, Weak};

use serde_json::json;

use super::adapter::McpSession;
use super::core::protocol::ToolContent;
use crate::{Tool, ToolError, ToolParameter, ToolResult};

/// A tool that proxies calls to an MCP server.
///
/// Created by [`McpSession::list_tools`](super::adapter::McpSession::list_tools) or
/// manually from an MCP tool description.
#[derive(Debug)]
pub struct DynamicTool {
    name: String,
    description: String,
    parameters: Vec<ToolParameter>,
    session: Weak<Mutex<McpSession>>,
}

impl DynamicTool {
    /// Creates a new dynamic tool from an MCP tool description and a shared session.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Vec<ToolParameter>,
        session: Arc<Mutex<McpSession>>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            session: Arc::downgrade(&session),
        }
    }
}

impl Tool for DynamicTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(self.parameters.clone())
    }

    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        let arguments: Option<serde_json::Value> =
            if input.trim().is_empty() {
                None
            } else {
                Some(crate::json_utils::parse(input).map_err(|e| {
                    ToolError::serialization(format!("invalid JSON arguments: {e}"))
                })?)
            };

        let arc = self.session.upgrade().ok_or_else(|| {
            ToolError::execution_failed("MCP session has been closed")
        })?;

        let mut session = arc
            .lock()
            .map_err(|e| ToolError::execution_failed(format!("session mutex poisoned: {e}")))?;

        let call_result = session
            .call_tool(&self.name, arguments)
            .map_err(|e| ToolError::execution_failed(format!("MCP call failed: {e}")))?;

        // Concatenate all text content items.
        let mut texts = Vec::new();
        for content in &call_result.content {
            if let ToolContent::Text { text } = content {
                texts.push(text.clone());
            }
        }

        Ok(ToolResult {
            success: !call_result.is_error,
            content: texts.join("\n"),
            metadata: Some(json!({"mcp_tool": self.name, "is_error": call_result.is_error})),
            elapsed_ms: None,
        })
    }
}
