use std::collections::HashMap;
use std::process::Stdio;

use serde_json::Value;
use tavern_core::{ContentPart, ToolError, ToolHandler, ToolResult};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const MAX_STDOUT_BYTES: usize = 10 * 1024 * 1024; // 10 MB
const MAX_STDERR_CHARS: usize = 4096;

/// 子进程工具执行器。通过 stdin/stdout JSON 协议与外部进程通信。
pub struct SubprocessHandler {
    command: String,
    timeout_ms: u64,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
}

impl SubprocessHandler {
    pub fn new(
        command: &str,
        timeout_ms: u64,
        cwd: Option<&str>,
        env: Option<&HashMap<String, String>>,
    ) -> Self {
        Self {
            command: command.to_string(),
            timeout_ms,
            cwd: cwd.map(|s| s.to_string()),
            env: env.cloned(),
        }
    }

    fn parse_command(&self) -> (&str, Vec<&str>) {
        let mut parts = self.command.split_whitespace();
        let prog = parts.next().unwrap_or("");
        let args: Vec<&str> = parts.collect();
        (prog, args)
    }
}

#[async_trait::async_trait]
impl ToolHandler for SubprocessHandler {
    async fn execute(
        &self,
        params: Value,
        tenant_id: &str,
        session_id: &str,
        tool_call_id: &str,
    ) -> Result<ToolResult, ToolError> {
        let request = serde_json::json!({
            "params": params,
            "tool_call_id": tool_call_id,
            "session_id": session_id,
            "tenant_id": tenant_id,
        });
        let request_json =
            serde_json::to_string(&request).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let (prog, args) = self.parse_command();
        if prog.is_empty() {
            return Ok(ToolResult {
                content: vec![ContentPart {
                    content_type: "text".into(),
                    text: Some("tool command is empty".into()),
                }],
                is_error: true,
                details: None,
            });
        }

        let mut cmd = Command::new(prog);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if let Some(ref cwd) = self.cwd {
            cmd.current_dir(cwd);
        }

        if let Some(env_map) = &self.env {
            cmd.env_clear();
            for (k, v) in env_map {
                cmd.env(k, v);
            }
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::ExecutionFailed(format!("failed to spawn: {}", e)))?;

        // Write stdin and close it to signal EOF to the child.
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(request_json.as_bytes())
                .await
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            drop(stdin);
        }

        // Start bounded async reads on stdout and stderr BEFORE waiting for the process.
        // This prevents deadlocks: the child process may block on writing to a full pipe
        // if no reader is consuming the output.
        let stdout_reader = child.stdout.take().map(|mut stdout| {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut limited = stdout.take((MAX_STDOUT_BYTES + 1) as u64);
                let _ = limited.read_to_end(&mut buf).await;
                buf
            })
        });

        let stderr_reader = child.stderr.take().map(|mut stderr| {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let _ = stderr.read_to_end(&mut buf).await;
                buf
            })
        });

        let timeout = std::time::Duration::from_millis(self.timeout_ms);
        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(Ok(s)) => Some(s),
            Ok(Err(e)) => {
                return Err(ToolError::ExecutionFailed(format!(
                    "child process error: {}",
                    e
                )));
            }
            Err(_) => {
                // kill_on_drop ensures child is killed on drop, but kill explicitly
                // so we can report the timeout before the drop cleanup.
                let _ = child.kill().await;
                None
            }
        };

        // Collect output from the reader tasks.
        let stdout = stdout_reader
            .unwrap_or_else(|| tokio::spawn(async { Vec::new() }))
            .await
            .unwrap_or_default();

        let stderr = stderr_reader
            .unwrap_or_else(|| tokio::spawn(async { Vec::new() }))
            .await
            .unwrap_or_default();

        if status.is_none() {
            return Ok(ToolResult {
                content: vec![ContentPart {
                    content_type: "text".into(),
                    text: Some(format!(
                        "tool execution timed out after {}ms",
                        self.timeout_ms
                    )),
                }],
                is_error: true,
                details: None,
            });
        }

        if stdout.len() > MAX_STDOUT_BYTES {
            return Ok(ToolResult {
                content: vec![ContentPart {
                    content_type: "text".into(),
                    text: Some(format!(
                        "tool output exceeded {}MB limit",
                        MAX_STDOUT_BYTES / (1024 * 1024)
                    )),
                }],
                is_error: true,
                details: None,
            });
        }

        let status = status.unwrap();

        if !status.success() {
            let stderr_str = String::from_utf8_lossy(&stderr);
            let stderr_truncated = if stderr_str.len() > MAX_STDERR_CHARS {
                format!("{}...(truncated)", &stderr_str[..MAX_STDERR_CHARS])
            } else {
                stderr_str.to_string()
            };
            return Ok(ToolResult {
                content: vec![ContentPart {
                    content_type: "text".into(),
                    text: Some(format!(
                        "tool exited with code {}: {}",
                        status.code().unwrap_or(-1),
                        stderr_truncated
                    )),
                }],
                is_error: true,
                details: None,
            });
        }

        if !stderr.is_empty() {
            tracing::debug!(
                stderr = %String::from_utf8_lossy(&stderr),
                "subprocess tool wrote to stderr"
            );
        }

        let stdout_str = String::from_utf8_lossy(&stdout);
        let result: serde_json::Value = match serde_json::from_str(&stdout_str) {
            Ok(v) => v,
            Err(e) => {
                return Ok(ToolResult {
                    content: vec![ContentPart {
                        content_type: "text".into(),
                        text: Some(format!("invalid JSON from tool: {}", e)),
                    }],
                    is_error: true,
                    details: None,
                });
            }
        };
        serde_json::from_value(result).map_err(|e| ToolError::ExecutionFailed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tavern_core::ToolHandler;

    #[tokio::test]
    async fn test_subprocess_success() {
        let handler = SubprocessHandler::new(
            "/bin/echo {\"content\":[{\"type\":\"text\",\"text\":\"hello\"}],\"is_error\":false}",
            5000,
            None,
            None,
        );
        let result = handler
            .execute(
                serde_json::json!({"query": "test"}),
                "tenant",
                "session",
                "call_id",
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content[0].text.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn test_subprocess_non_zero_exit() {
        let handler = SubprocessHandler::new("/usr/bin/false", 5000, None, None);
        let result = handler
            .execute(serde_json::json!({}), "t", "s", "c")
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content[0]
            .text
            .as_ref()
            .unwrap()
            .contains("exited with code"));
    }

    #[tokio::test]
    async fn test_subprocess_empty_command() {
        let handler = SubprocessHandler::new("", 5000, None, None);
        let result = handler
            .execute(serde_json::json!({}), "t", "s", "c")
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content[0]
            .text
            .as_ref()
            .unwrap()
            .contains("empty"));
    }

    #[tokio::test]
    async fn test_subprocess_invalid_json_stdout() {
        let handler =
            SubprocessHandler::new("/bin/echo not valid json at all", 5000, None, None);
        let result = handler
            .execute(serde_json::json!({}), "t", "s", "c")
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content[0]
            .text
            .as_ref()
            .unwrap()
            .contains("JSON"));
    }
}
