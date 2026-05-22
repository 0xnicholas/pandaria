use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

const BASH_TIMEOUT_SECS: u64 = 30;

/// Result of executing a bash command.
#[derive(Debug, Clone)]
pub struct BashResult {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Detect if input is a bash mode command.
pub fn detect_bash_mode(input: &str) -> Option<&str> {
    let trimmed = input.trim();
    if trimmed.starts_with("!!") {
        Some(trimmed.strip_prefix("!!").unwrap_or("").trim_start())
    } else if trimmed.starts_with('!') {
        Some(trimmed.strip_prefix('!').unwrap_or("").trim_start())
    } else {
        None
    }
}

/// Returns true if the input uses double-bang (`!!`) which means
/// the command and its output should be displayed in chat.
pub fn is_double_bang(input: &str) -> bool {
    input.trim().starts_with("!!")
}

/// Execute a shell command asynchronously with timeout.
pub async fn execute_bash(command: &str) -> BashResult {
    let cmd_str = command.to_string();
    let result = timeout(
        Duration::from_secs(BASH_TIMEOUT_SECS),
        Command::new("sh")
            .arg("-c")
            .arg(&cmd_str)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => BashResult {
            command: cmd_str,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
            timed_out: false,
        },
        Ok(Err(e)) => BashResult {
            command: cmd_str,
            stdout: String::new(),
            stderr: format!("Failed to execute: {}", e),
            exit_code: None,
            timed_out: false,
        },
        Err(_) => BashResult {
            command: cmd_str,
            stdout: String::new(),
            stderr: format!("Command timed out after {} seconds", BASH_TIMEOUT_SECS),
            exit_code: None,
            timed_out: true,
        },
    }
}

/// Format bash result as text to send to the backend.
/// For `!command`, only stdout is sent as the user message content.
/// For `!!command`, both command and output are included.
pub fn format_for_send(result: &BashResult, show_command: bool) -> String {
    let mut text = String::new();
    if show_command {
        text.push_str(&format!("$ {}\n\n", result.command));
    }
    if !result.stdout.is_empty() {
        text.push_str(&result.stdout);
        if !result.stdout.ends_with('\n') {
            text.push('\n');
        }
    }
    if !result.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("stderr:\n");
        text.push_str(&result.stderr);
        if !result.stderr.ends_with('\n') {
            text.push('\n');
        }
    }
    if result.timed_out {
        text.push_str(&format!("\n[timed out after {}s]\n", BASH_TIMEOUT_SECS));
    }
    if let Some(code) = result.exit_code {
        text.push_str(&format!("\n[exit code: {}]\n", code));
    }
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_bash_mode() {
        assert_eq!(detect_bash_mode("!ls"), Some("ls"));
        assert_eq!(detect_bash_mode("!!pwd"), Some("pwd"));
        assert_eq!(detect_bash_mode("  !echo hi"), Some("echo hi"));
        assert_eq!(detect_bash_mode("normal text"), None);
        assert_eq!(detect_bash_mode("!"), Some(""));
    }

    #[test]
    fn test_is_double_bang() {
        assert!(is_double_bang("!!ls"));
        assert!(!is_double_bang("!ls"));
    }

    #[tokio::test]
    async fn test_execute_bash_echo() {
        let result = execute_bash("echo hello").await;
        assert!(result.stdout.contains("hello"));
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_execute_bash_stderr() {
        let result = execute_bash("echo error >&2; exit 1").await;
        assert!(result.stderr.contains("error"));
        assert_eq!(result.exit_code, Some(1));
    }
}
