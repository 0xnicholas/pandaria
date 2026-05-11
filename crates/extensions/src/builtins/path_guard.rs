use std::collections::HashMap;

use async_trait::async_trait;

use agent_core::context::{ToolCallCtx, ToolResultCtx};
use agent_core::mutations::{HookDecision, ToolCallMutation, ToolResultMutation};

use crate::host::extension::Extension;

/// 规范化路径（纯字符串操作，不访问文件系统）。
///
/// 去除 `.` 和 `..`，处理多余的 `/`。
/// 如果绝对路径尝试逃逸根目录（如 `/../../../etc/passwd`），返回 `None`。
fn normalize_path(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }

    let is_absolute = path.starts_with('/');
    let mut parts = Vec::new();

    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                if parts.pop().is_none() && is_absolute {
                    // 绝对路径尝试逃逸根目录
                    return None;
                }
            }
            _ => parts.push(part),
        }
    }

    let normalized = if is_absolute {
        format!("/{}", parts.join("/"))
    } else {
        parts.join("/")
    };

    Some(normalized)
}

/// 检查路径是否在允许的 workspace 范围内。
///
/// 规则：
/// - 绝对路径：规范化后必须以 `/workspace/{tenant_id}/` 开头
/// - 相对路径：默认允许（假设工具执行时 cwd 为 workspace）
/// - 路径逃逸（`..` 越界）：拒绝
fn is_path_allowed(path: &str, tenant_id: &str) -> bool {
    let normalized = match normalize_path(path) {
        Some(p) => p,
        None => return false,
    };

    // 相对路径默认允许
    if !normalized.starts_with('/') {
        return true;
    }

    let allowed_prefix = format!("/workspace/{}/", tenant_id);
    normalized.starts_with(&allowed_prefix)
}

/// PathGuard extension — 文件系统路径校验。
///
/// 确保文件操作只能访问 `/workspace/{tenant_id}/` 下的路径。
/// 支持按工具名精确配置字段，或对未配置工具递归扫描所有字符串值。
pub struct PathGuardExtension {
    tool_fields: HashMap<String, Vec<String>>,
    scan_unknown_tools: bool,
}

impl PathGuardExtension {
    /// 创建 PathGuard。
    ///
    /// # 参数
    /// - `tool_fields`: `tool_name -> [field_name]`，精确指定需要检查路径的字段
    /// - `scan_unknown_tools`: 对未在配置中出现的工具，是否递归扫描所有字符串值
    pub fn new(tool_fields: HashMap<String, Vec<String>>, scan_unknown_tools: bool) -> Self {
        Self {
            tool_fields,
            scan_unknown_tools,
        }
    }

    fn extract_paths(&self, tool_name: &str, value: &serde_json::Value, paths: &mut Vec<String>) {
        match self.tool_fields.get(tool_name) {
            Some(fields) => {
                for field in fields {
                    if let Some(v) = value.get(field) {
                        Self::collect_string_paths(v, paths);
                    }
                }
            }
            None if self.scan_unknown_tools => {
                Self::collect_string_paths(value, paths);
            }
            _ => {}
        }
    }

    fn collect_string_paths(value: &serde_json::Value, paths: &mut Vec<String>) {
        match value {
            serde_json::Value::String(s) => {
                if s.starts_with('/') || s.starts_with('.') || s.contains('/') {
                    paths.push(s.clone());
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr {
                    Self::collect_string_paths(v, paths);
                }
            }
            serde_json::Value::Object(obj) => {
                for (_, v) in obj {
                    Self::collect_string_paths(v, paths);
                }
            }
            _ => {}
        }
    }
}

#[async_trait]
impl Extension for PathGuardExtension {
    fn name(&self) -> &str {
        "path-guard"
    }

    async fn on_tool_call(
        &self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        let mut paths = Vec::new();
        self.extract_paths(&ctx.tool_name, &ctx.input, &mut paths);

        for path in &paths {
            if !is_path_allowed(path, &ctx.tenant_id) {
                tracing::warn!(
                    target: "pandaria.path_guard",
                    tenant_id = %ctx.tenant_id,
                    session_id = %ctx.session_id,
                    tool_name = %ctx.tool_name,
                    path = %path,
                    action = "block_illegal_path"
                );
                return (
                    HookDecision::Block {
                        reason: format!(
                            "path '{}' is outside of allowed workspace (/workspace/{}/)",
                            path, ctx.tenant_id
                        ),
                    },
                    ToolCallMutation::default(),
                );
            }
        }

        (HookDecision::Continue, ToolCallMutation::default())
    }

    async fn on_tool_result(
        &self,
        ctx: &ToolResultCtx,
    ) -> ToolResultMutation {
        let mut paths = Vec::new();

        // 检查 details 中的路径泄露
        if let Some(details) = &ctx.details {
            Self::collect_string_paths(details, &mut paths);
        }

        for path in &paths {
            if !is_path_allowed(path, &ctx.tenant_id) {
                tracing::warn!(
                    target: "pandaria.path_guard",
                    tenant_id = %ctx.tenant_id,
                    session_id = %ctx.session_id,
                    tool_name = %ctx.tool_name,
                    path = %path,
                    action = "leak_illegal_path"
                );
                return ToolResultMutation {
                    content: Some(vec![llm_client::Content::Text {
                        text: "[PathGuard: illegal path reference removed]".to_string(),
                        text_signature: None,
                    }]),
                    details: None,
                    is_error: Some(true),
                    terminate: None,
                };
            }
        }

        ToolResultMutation::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path("/workspace/t1/foo.txt"), Some("/workspace/t1/foo.txt".to_string()));
        assert_eq!(normalize_path("/workspace/t1/../foo.txt"), Some("/workspace/foo.txt".to_string()));
        assert_eq!(normalize_path("/workspace/t1/./foo.txt"), Some("/workspace/t1/foo.txt".to_string()));
        assert_eq!(normalize_path("/etc/passwd"), Some("/etc/passwd".to_string()));
        assert_eq!(normalize_path("/../../../etc/passwd"), None);
        assert_eq!(normalize_path("foo/bar.txt"), Some("foo/bar.txt".to_string()));
    }

    #[test]
    fn test_is_path_allowed() {
        assert!(is_path_allowed("/workspace/t1/foo.txt", "t1"));
        assert!(is_path_allowed("/workspace/t1/subdir/bar.txt", "t1"));
        assert!(!is_path_allowed("/workspace/t2/foo.txt", "t1"));
        assert!(!is_path_allowed("/etc/passwd", "t1"));
        assert!(!is_path_allowed("/../../../etc/passwd", "t1"));
        assert!(is_path_allowed("foo.txt", "t1")); // 相对路径默认允许
    }

    #[tokio::test]
    async fn test_path_guard_blocks_escape() {
        let mut fields = HashMap::new();
        fields.insert("read_file".to_string(), vec!["path".to_string()]);
        let guard = PathGuardExtension::new(fields, false);

        let ctx = ToolCallCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            tool_name: "read_file".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({"path": "/etc/passwd"}),
        };

        let (decision, _) = guard.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Block { .. }));
    }

    #[tokio::test]
    async fn test_path_guard_allows_legal_path() {
        let mut fields = HashMap::new();
        fields.insert("read_file".to_string(), vec!["path".to_string()]);
        let guard = PathGuardExtension::new(fields, false);

        let ctx = ToolCallCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            tool_name: "read_file".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({"path": "/workspace/t1/project/main.rs"}),
        };

        let (decision, _) = guard.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Continue));
    }

    #[tokio::test]
    async fn test_path_guard_scan_unknown_tools() {
        let guard = PathGuardExtension::new(HashMap::new(), true);

        let ctx = ToolCallCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            tool_name: "custom_tool".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({"some_field": "/etc/shadow"}),
        };

        let (decision, _) = guard.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Block { .. }));
    }

    #[tokio::test]
    async fn test_path_guard_leak_in_result() {
        let guard = PathGuardExtension::new(HashMap::new(), false);

        let ctx = ToolResultCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            tool_name: "read_file".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({}),
            content: vec![],
            details: Some(serde_json::json!({"path": "/etc/passwd"})),
            is_error: false,
        };

        let mutation = guard.on_tool_result(&ctx).await;
        assert!(mutation.is_error.unwrap_or(false));
    }
}
