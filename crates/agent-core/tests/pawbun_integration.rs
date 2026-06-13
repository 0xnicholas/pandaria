use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use serde_json::json;
use agent_core::tools::pawbun_adapter::PawbunToolAdapter;
use agent_core::types::{AgentTool, AgentToolProgressUpdate, AgentToolResult};
use pawbun_toolkit::{FileReadTool, FileWriteTool, DirectoryListTool};
use ai_provider::Content;

fn setup_dir(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("pandaria_pawbun_integration")
        .join(test_name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn content_text(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

// ── file_read ──

#[tokio::test]
async fn test_file_read_success() {
    let dir = setup_dir("file_read_success");
    std::fs::write(dir.join("hello.txt"), "hello world").unwrap();

    let adapter = PawbunToolAdapter::new(Box::new(FileReadTool::new(&dir)));
    let result = adapter
        .execute(
            "call_1",
            json!({"path": "hello.txt"}),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert!(!result.is_error, "should succeed: {:?}", result);
    assert_eq!(content_text(&result), "hello world");
}

#[tokio::test]
async fn test_file_read_path_traversal() {
    let dir = setup_dir("file_read_traversal");
    let adapter = PawbunToolAdapter::new(Box::new(FileReadTool::new(&dir)));
    let result = adapter
        .execute(
            "call_1",
            json!({"path": "../etc/passwd"}),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert!(result.is_error, "path traversal should be blocked");
    let text = content_text(&result);
    assert!(
        text.contains("path traversal") || text.contains("invalid path"),
        "expected traversal error, got: {text}"
    );
}

#[tokio::test]
async fn test_file_read_bad_type() {
    let dir = setup_dir("file_read_bad_type");
    let adapter = PawbunToolAdapter::new(Box::new(FileReadTool::new(&dir)));
    let result = adapter
        .execute(
            "call_1",
            json!({"path": 42}), // wrong type: int instead of string
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert!(result.is_error, "bad type should error");
}

// ── file_write + read round-trip ──

#[tokio::test]
async fn test_file_write_and_read() {
    let dir = setup_dir("file_write_read");
    let write_adapter = PawbunToolAdapter::new(Box::new(FileWriteTool::new(&dir)));
    let read_adapter = PawbunToolAdapter::new(Box::new(FileReadTool::new(&dir)));

    // Write
    let w = write_adapter
        .execute(
            "call_w",
            json!({"path": "out.txt", "content": "round-trip data"}),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!w.is_error, "write failed: {w:?}");

    // Read back
    let r = read_adapter
        .execute(
            "call_r",
            json!({"path": "out.txt"}),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!r.is_error, "read failed: {r:?}");
    assert_eq!(content_text(&r), "round-trip data");
}

// ── directory_list ──

#[tokio::test]
async fn test_directory_list() {
    let dir = setup_dir("directory_list");
    std::fs::File::create(dir.join("a.txt")).unwrap();
    std::fs::create_dir(dir.join("subdir")).unwrap();

    let adapter = PawbunToolAdapter::new(Box::new(DirectoryListTool::new(&dir)));
    let result = adapter
        .execute(
            "call_1",
            json!({"path": "."}),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let content = content_text(&result);
    let items: Vec<serde_json::Value> = serde_json::from_str(&content).unwrap();
    assert_eq!(items.len(), 2);
    let names: Vec<&str> = items.iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"a.txt"));
    assert!(names.contains(&"subdir"));
}

// ── path_guard integration ──

#[tokio::test]
async fn test_path_guard_blocks_etc_passwd() {
    use agent_core::harness::config::HookConfig;
    use agent_core::hook::default_dispatcher::DefaultHookDispatcher;
    use agent_core::hook::dispatcher::HookDispatcher;
    use agent_core::hook::context::ToolCallCtx;
    use agent_core::hook::mutations::HookDecision;

    let config = HookConfig::default().with_pawbun_defaults();
    let dispatcher =
        DefaultHookDispatcher::from_config(agent_core::space::AgentSpace::default(), &config);

    let mut ctx = ToolCallCtx::new("t1", "s1", "file_read", "call_1");
    ctx.input = json!({"path": "/etc/passwd"});

    let (decision, _) = dispatcher.on_tool_call(&ctx).await;
    match decision {
        HookDecision::Block { reason } => {
            assert!(
                reason.contains("path") || reason.contains("forbidden"),
                "expected path-related block reason, got: {reason}"
            );
        }
        HookDecision::Continue => {
            panic!("path_guard should block /etc/passwd for file_read");
        }
    }
}

// ── code_execute ──

#[tokio::test]
async fn test_code_execute_placeholder() {
    use pawbun_toolkit::CodeExecuteTool;

    let _dir = setup_dir("code_execute_placeholder");
    let adapter = PawbunToolAdapter::new(Box::new(CodeExecuteTool));

    let result = adapter
        .execute(
            "call_1",
            json!({"code": "echo hello", "language": "bash"}),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    // CodeExecuteTool is currently a placeholder — it returns an error
    assert!(result.is_error, "placeholder should return error");
    let text = content_text(&result);
    assert!(
        text.contains("placeholder") || text.contains("sandbox"),
        "expected placeholder message, got: {text}"
    );
}
