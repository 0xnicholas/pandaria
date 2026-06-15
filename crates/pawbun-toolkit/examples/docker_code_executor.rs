//! Docker 沙箱代码执行适配器示例。
//!
//! 此示例演示如何为 `CodeExecuteTool` 提供一个可运行的 Docker 实现。
//! 它通过调用本地 `docker` CLI 在隔离容器中执行代码。
//!
//! # 运行前提
//! - Docker Engine / Docker Desktop 已安装并运行
//! - 当前用户有权限执行 `docker` 命令
//!
//! # 运行
//! ```bash
//! cargo run --example docker_code_executor --features tokio
//! ```

use async_trait::async_trait;
use pawbun_toolkit::{AsyncTool, AsyncToolExecutor, Tool, ToolError, ToolParameter, ToolResult};
use serde_json::json;
use std::borrow::Cow;
use std::collections::HashMap;
use std::time::Duration;
use tokio::process::Command;

/// Docker 沙箱代码执行器。
///
/// 通过 `docker` CLI 在隔离容器中执行用户代码，支持超时强制终止。
#[derive(Debug)]
pub struct DockerCodeExecutor {
    allowed_images: Vec<String>,
    default_timeout_ms: u64,
    memory_limit_mb: u64,
    /// 语言到 Docker 镜像的映射。
    image_map: HashMap<String, String>,
}

impl DockerCodeExecutor {
    pub fn new() -> Self {
        let mut image_map = HashMap::new();
        image_map.insert("python".into(), "python:3.12-slim".into());
        image_map.insert("rust".into(), "rust:1.75-slim".into());
        image_map.insert("node".into(), "node:20-slim".into());

        Self {
            allowed_images: image_map.values().cloned().collect(),
            default_timeout_ms: 30_000,
            memory_limit_mb: 128,
            image_map,
        }
    }

    pub fn with_allowed_images(mut self, images: Vec<String>) -> Self {
        self.allowed_images = images;
        self
    }

    pub fn with_timeout(mut self, ms: u64) -> Self {
        self.default_timeout_ms = ms;
        self
    }

    pub fn with_memory_limit(mut self, mb: u64) -> Self {
        self.memory_limit_mb = mb;
        self
    }

    fn parse_input(input: &str) -> Result<(serde_json::Value, u64), ToolError> {
        let v: serde_json::Value = serde_json::from_str(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid JSON: {e}")))?;

        let timeout = v
            .get("timeout_ms")
            .and_then(|t| t.as_u64())
            .unwrap_or(30_000);

        Ok((v, timeout))
    }
}

impl Default for DockerCodeExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for DockerCodeExecutor {
    fn name(&self) -> &str {
        "code_execute"
    }

    fn description(&self) -> &str {
        "Execute code in a Docker sandbox."
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "code".into(),
                description: "Source code to execute".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
            ToolParameter {
                name: "language".into(),
                description: "Programming language (python, rust, node)".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
            ToolParameter {
                name: "timeout_ms".into(),
                description: "Execution timeout in milliseconds".into(),
                required: false,
                schema: json!({"type": "integer"}),
            },
        ])
    }

    fn execute(&self, _input: &str) -> Result<ToolResult, ToolError> {
        Err(ToolError::execution_failed(
            "DockerCodeExecutor requires async execution. Use execute_async instead.",
        ))
    }

    fn as_async(&self) -> Option<&dyn AsyncTool> {
        Some(self)
    }
}

#[async_trait]
impl AsyncTool for DockerCodeExecutor {
    async fn execute_async(
        &self,
        input: &str,
    ) -> Result<ToolResult, ToolError> {
        let (parsed, timeout_ms) = Self::parse_input(input)?;

        let code = parsed
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'code' field"))?;

        let language = parsed
            .get("language")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'language' field"))?;

        let image = self
            .image_map
            .get(language)
            .ok_or_else(|| {
                ToolError::invalid_input(format!(
                    "unsupported language: {language}. Supported: {:?}",
                    self.image_map.keys().collect::<Vec<_>>()
                ))
            })?;

        if !self.allowed_images.contains(image) {
            return Err(ToolError::invalid_input(format!(
                "image {image} is not in the allowed whitelist"
            )));
        }

        // 安全：限制容器资源
        let memory = format!("{}m", self.memory_limit_mb);

        // 创建临时目录，将代码写入文件后通过只读卷挂载到容器。
        // 避免通过环境变量或命令行参数传递代码，消除命令注入风险。
        let tmp_dir = {
            let suffix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            std::env::temp_dir().join(format!("pawbun-docker-{suffix}"))
        };
        tokio::fs::create_dir_all(&tmp_dir)
            .await
            .map_err(|e| {
                ToolError::execution_failed(format!("failed to create temp dir: {e}"))
                    .with_source(e)
            })?;

        let (ext, cmd) = match language {
            "python" => ("py", "python /code/main.py"),
            "rust" => ("rs", "rustc /code/main.rs -o /tmp/main && /tmp/main"),
            "node" => ("js", "node /code/main.js"),
            _ => ("txt", "cat /code/main.txt"),
        };

        let src_path = tmp_dir.join(format!("main.{ext}"));
        tokio::fs::write(&src_path, code)
            .await
            .map_err(|e| {
                ToolError::execution_failed(format!("failed to write code to temp file: {e}"))
                    .with_source(e)
            })?;

        let timeout = Duration::from_millis(timeout_ms);

        let child = Command::new("docker")
            .arg("run")
            .arg("--rm")
            .arg("--network=none")
            .arg("--read-only")
            .arg(format!("--memory={memory}"))
            .arg(format!("--memory-swap={memory}"))
            .arg("-v")
            .arg(format!("{}:/code:ro", tmp_dir.display()))
            .arg("--tmpfs")
            .arg("/tmp:rw,noexec,nosuid,size=50m")
            .arg("--entrypoint")
            .arg("sh")
            .arg(image)
            .arg("-c")
            .arg(cmd)
            .kill_on_drop(true)
            .output();

        let output = tokio::time::timeout(timeout, child)
            .await
            .map_err(|_| ToolError::Timeout(timeout_ms))?
            .map_err(|e| {
                ToolError::execution_failed(format!("docker command failed: {e}"))
                    .with_source(e)
            })?;

        // 无论成功或失败，都尝试清理临时目录
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let success = output.status.success();
        let content = if success {
            stdout.to_string()
        } else {
            format!("stdout:\n{stdout}\n\nstderr:\n{stderr}")
        };

        Ok(ToolResult {
            success,
            content,
            metadata: Some(json!({
                "language": language,
                "image": image,
                "timeout_ms": timeout_ms,
            })),
            elapsed_ms: None,
        })
    }
}

#[tokio::main]
async fn main() {
    let executor = DockerCodeExecutor::new()
        .with_timeout(10_000)
        .with_memory_limit(64);

    let mut toolkit = pawbun_toolkit::ToolKit::new();
    toolkit.register(Box::new(executor));

    // 示例：执行 Python 代码
    let input = json!({
        "code": "print(1 + 1)",
        "language": "python"
    })
    .to_string();

    match toolkit.execute_async("code_execute", &input, &pawbun_toolkit::TokioExecutor).await {
        Ok(result) => {
            println!("Success: {}", result.success);
            println!("Output:\n{}", result.content);
        }
        Err(e) => {
            eprintln!("Error: {e}");
        }
    }
}
