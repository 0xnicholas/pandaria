use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::process::Command;

use crate::tools::path_utils::resolve_sandbox_path;
use crate::{AsyncTool, Tool, ToolError, ToolParameter, ToolResult};

/// 基于 subprocess 的本地代码执行器。
///
/// 通过 `bash -c` 在沙箱工作目录中执行 shell 命令。
/// 适用于本地开发和信任环境。生产环境请使用 DockerCodeExecutor。
///
/// # Example
/// ```
/// use pawbun_toolkit::{LocalCodeExecutor, Tool};
/// use std::time::Duration;
///
/// let dir = std::env::temp_dir();
/// let executor = LocalCodeExecutor::new(&dir)
///     .with_timeout(Duration::from_secs(5));
/// assert_eq!(executor.name(), "code_execute");
/// ```
#[derive(Debug)]
pub struct LocalCodeExecutor {
    /// 沙箱工作目录（所有命令在此执行）。
    pub work_dir: PathBuf,
    /// 执行超时（默认 30 秒）。
    pub timeout: Duration,
    /// 允许的命令白名单。空 vec 表示允许所有命令。
    pub allowed_commands: Vec<String>,
}

impl LocalCodeExecutor {
    /// 创建一个新的 `LocalCodeExecutor`。
    ///
    /// `base_dir` 为沙箱根目录，所有命令将在此目录内执行。
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            work_dir: base_dir.into(),
            timeout: Duration::from_secs(30),
            allowed_commands: Vec::new(),
        }
    }

    /// 设置执行超时（默认 30 秒）。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 设置允许的命令白名单。空 vec 表示允许所有命令。
    pub fn with_allowed_commands(mut self, cmds: Vec<String>) -> Self {
        self.allowed_commands = cmds;
        self
    }
}

impl Tool for LocalCodeExecutor {
    fn name(&self) -> &str {
        "code_execute"
    }

    fn description(&self) -> &str {
        "Execute a shell command via bash in the sandboxed workspace."
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "command".into(),
                description: "Shell command to execute via bash -c".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
            ToolParameter {
                name: "work_dir".into(),
                description: "Working directory relative to sandbox root".into(),
                required: false,
                schema: json!({"type": "string"}),
            },
            ToolParameter {
                name: "timeout_ms".into(),
                description: "Execution timeout in milliseconds (default 30000)".into(),
                required: false,
                schema: json!({"type": "integer"}),
            },
        ])
    }

    fn execute(&self, _input: &str) -> Result<ToolResult, ToolError> {
        Err(ToolError::execution_failed(
            "LocalCodeExecutor requires async execution. Use execute_async instead.",
        ))
    }

    fn as_async(&self) -> Option<&dyn AsyncTool> {
        Some(self)
    }
}

#[async_trait]
impl AsyncTool for LocalCodeExecutor {
    async fn execute_async(&self, input: &str) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value = serde_json::from_str(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid JSON: {e}")))?;

        let command = parsed["command"]
            .as_str()
            .ok_or_else(|| ToolError::invalid_input("missing 'command' field"))?;

        // 1. 白名单校验
        if !self.allowed_commands.is_empty() {
            let cmd_name = command.split_whitespace().next().unwrap_or("");
            if !self.allowed_commands.iter().any(|a| a == cmd_name) {
                return Err(ToolError::invalid_input(format!(
                    "command '{cmd_name}' not in allowed list"
                )));
            }
        }

        // 2. 解析 work_dir（可选）—— 路径沙箱
        let work_dir = if let Some(sub) = parsed["work_dir"].as_str() {
            resolve_sandbox_path(Some(&self.work_dir), sub)?
        } else {
            self.work_dir.clone()
        };

        // 3. 解析超时
        let timeout_ms = parsed["timeout_ms"]
            .as_u64()
            .unwrap_or(self.timeout.as_millis() as u64);
        let timeout = Duration::from_millis(timeout_ms);

        // 4. 使用 tokio::process::Command 实现真正的异步执行。
        //    超时通过 tokio::time::timeout 处理。
        let work_dir_clone = work_dir.clone();
        let cmd = command.to_string();

        let start = std::time::Instant::now();

        let child = Command::new("bash")
            .arg("-c")
            .arg(&cmd)
            .current_dir(&work_dir_clone)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                ToolError::execution_failed(format!("spawn failed: {e}")).with_source(e)
            })?;

        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return Err(ToolError::execution_failed(format!("wait failed: {e}"))
                    .with_source(e));
            }
            Err(_elapsed) => {
                return Err(ToolError::Timeout(timeout_ms));
            }
        };

        let elapsed = start.elapsed().as_millis() as u64;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let success = output.status.success();

        let content = if success {
            stdout.trim().to_string()
        } else {
            format!(
                "exit code: {}\nstdout:\n{}\nstderr:\n{}",
                output.status.code().unwrap_or(-1),
                stdout.trim(),
                stderr.trim()
            )
        };

        Ok(ToolResult {
            success,
            content,
            metadata: Some(json!({
                "exit_code": output.status.code(),
                "elapsed_ms": elapsed,
                "work_dir": work_dir_clone.to_string_lossy(),
            })),
            elapsed_ms: Some(elapsed),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_executor() -> LocalCodeExecutor {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("pawbun_ce_{}", id));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        LocalCodeExecutor::new(&dir).with_timeout(Duration::from_secs(5))
    }

    #[test]
    fn test_name_and_description() {
        let e = make_executor();
        assert_eq!(e.name(), "code_execute");
        assert!(e.description().contains("shell command"));
    }

    #[test]
    fn test_parameters_schema() {
        let e = make_executor();
        let params = e.parameters();
        assert_eq!(params.len(), 3);
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"command"));
        assert!(names.contains(&"work_dir"));
        assert!(names.contains(&"timeout_ms"));
    }

    #[tokio::test]
    async fn test_execute_simple_command() {
        let e = make_executor();
        let result = e
            .execute_async(r#"{"command": "echo hello"}"#)
            .await
            .unwrap();
        assert!(result.success, "echo should succeed: {:?}", result);
        assert!(
            result.content.contains("hello"),
            "expected 'hello' in: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_execute_failing_command() {
        let e = make_executor();
        let result = e
            .execute_async(r#"{"command": "ls /nonexistent_xyz"}"#)
            .await
            .unwrap();
        assert!(!result.success, "ls of nonexistent should fail");
        assert!(
            result.content.contains("exit code"),
            "should show exit code: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let dir = std::env::temp_dir().join("pawbun_timeout_test");
        let _ = std::fs::create_dir_all(&dir);
        let e =
            LocalCodeExecutor::new(&dir).with_timeout(Duration::from_millis(500));
        let result = e.execute_async(r#"{"command": "sleep 60"}"#).await;
        assert!(result.is_err(), "sleep 60 should timeout");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timeout"),
            "expected timeout error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_execute_allowed_commands() {
        let dir = std::env::temp_dir().join("pawbun_allowed_test");
        let _ = std::fs::create_dir_all(&dir);
        let e = LocalCodeExecutor::new(&dir)
            .with_allowed_commands(vec!["echo".into()])
            .with_timeout(Duration::from_secs(5));
        // echo is allowed
        let r = e.execute_async(r#"{"command": "echo ok"}"#).await.unwrap();
        assert!(r.success);
        // ls is NOT allowed
        let r = e.execute_async(r#"{"command": "ls"}"#).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn test_execute_missing_command() {
        let e = make_executor();
        let result = e.execute_async(r#"{}"#).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("missing 'command'"), "got: {err}");
    }

    #[tokio::test]
    async fn test_execute_work_dir_subdirectory() {
        let e = make_executor();
        std::fs::create_dir_all(e.work_dir.join("sub")).unwrap();
        let result = e
            .execute_async(r#"{"command": "pwd", "work_dir": "sub"}"#)
            .await
            .unwrap();
        assert!(result.success);
        assert!(
            result.content.contains("sub"),
            "pwd should show sub dir: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_execute_work_dir_traversal() {
        let e = make_executor();
        let result = e
            .execute_async(r#"{"command": "pwd", "work_dir": "../"}"#)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("path traversal") || err.contains("invalid path"),
            "expected traversal error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_execute_invalid_json() {
        let e = make_executor();
        let result = e.execute_async("not json").await;
        assert!(result.is_err());
    }
}
