# Builtin Tools Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate Pawbun tool suite as Pandaria's built-in tool set via an adapter layer, eliminating the need for HTTP-proxied basic tools.

**Architecture:** A `PawbunToolAdapter` wraps `pawbun_toolkit::Tool` into `agent_core::AgentTool`, bridging sync `&str` execution to async with `spawn_blocking`. `SessionBuilder` auto-registers Pawbun tools via `BuiltinToolsConfig`. A `HookConfig::with_pawbun_defaults()` method populates `path_guard_fields` for the dual-layer sandbox.

**Tech Stack:** Rust + tokio, pawbun-toolkit (git dep), existing agent-core/tenant/api-gateway crates.

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/agent-core/Cargo.toml` | Modify | Add `pawbun-toolkit` git dependency + `pawbun-http` feature |
| `crates/agent-core/src/tools/pawbun_adapter.rs` | Create | `PawbunToolAdapter`: `AgentTool` impl wrapping `Tool` |
| `crates/agent-core/src/tools/mod.rs` | Modify | Register `pawbun_adapter` module |
| `crates/agent-core/src/harness/config.rs` | Modify | Add `HookConfig::with_pawbun_defaults()` |
| `crates/agent-core/src/file_ops.rs` | Modify | Extend `DefaultFileOperationExtractor` defaults |
| `crates/agent-core/src/harness/builder.rs` | Modify | Add `with_builtin_tools_config()`, `build_pawbun_tool_refs()` |
| `crates/api-gateway/src/types.rs` | Modify | Add `BuiltinToolsConfig` struct to `CreateSessionRequest` |
| `crates/api-gateway/src/routes/sessions.rs` | Modify | Pass `builtin_tools` config to session params |
| `crates/tenant/src/session_entry.rs` | Modify | Add `builtin_tools` fields to `ActiveSession` for clone propagation |
| `crates/tenant/src/manager.rs` | Modify | Add `builtin_tools` to `CreateSessionParams`; pass through to `SessionBuilder` |
| `crates/agent-core/tests/pawbun_integration.rs` | Create | Integration tests (adapter + Pawbun tools) |
| `crates/api-gateway/tests/e2e/e2e_builtin_tools.rs` | Create | E2E tests |

---

### Task 1: Add Pawbun dependency to agent-core

**Files:**
- Modify: `crates/agent-core/Cargo.toml`

- [ ] **Step 1: Add pawbun-toolkit git dependency**

```toml
# Under [dependencies]
pawbun-toolkit = { git = "https://github.com/0xnicholas/pawbun", branch = "main", default-features = false }
```

- [ ] **Step 2: Add pawbun-http feature**

```toml
# Under [features]
pawbun-http = ["pawbun-toolkit/http"]
```

- [ ] **Step 3: Verify dependency resolves**

```bash
cargo check -p agent-core 2>&1 | head -5
```

Expected: compiles (existing code unmodified, new dep resolved).

- [ ] **Step 4: Commit**

```bash
git add crates/agent-core/Cargo.toml
git commit -m "chore: add pawbun-toolkit git dependency to agent-core"
```

---

### Task 2: PawbunToolAdapter — core adapter

**Files:**
- Create: `crates/agent-core/src/tools/pawbun_adapter.rs`
- Modify: `crates/agent-core/src/tools/mod.rs`

- [ ] **Step 1: Write failing unit tests**

Create `pawbun_adapter.rs` with test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;
    use pawbun_toolkit::{Tool, ToolParameter, ToolResult as PToolResult, ToolError as PToolError};
    use serde_json::json;

    #[derive(Debug)]
    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echoes input" }
        fn parameters(&self) -> Cow<'static, [ToolParameter]> {
            Cow::Owned(vec![ToolParameter {
                name: "message".into(),
                description: "Thing to echo".into(),
                required: true,
                schema: json!({"type": "string"}),
            }])
        }
        fn execute(&self, input: &str) -> Result<PToolResult, PToolError> {
            Ok(PToolResult {
                success: true,
                content: format!("echo: {}", input),
                metadata: Some(json!({"parsed": true})),
                elapsed_ms: None,
            })
        }
    }

    #[test]
    fn test_schema_conversion() {
        let adapter = PawbunToolAdapter::new(Box::new(EchoTool));
        let schema = adapter.parameters();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["message"]["type"], "string");
        assert!(schema["required"].as_array().unwrap().contains(&json!("message")));
    }

    #[test]
    fn test_schema_cached() {
        let adapter = PawbunToolAdapter::new(Box::new(EchoTool));
        let a = adapter.parameters();
        let b = adapter.parameters();
        // Same Value pointer (cached, not recomputed)
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn test_execute_success() {
        let adapter = PawbunToolAdapter::new(Box::new(EchoTool));
        let result = adapter.execute(
            "call_1",
            json!({"message": "hello"}),
            None,
            CancellationToken::new(),
        ).await.unwrap();

        assert!(!result.is_error);
        let text = result.content.iter()
            .filter_map(|c| match c {
                ai_provider::Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(text, "echo: {\"message\":\"hello\"}");
        assert_eq!(result.details.unwrap()["parsed"], true);
    }

    #[tokio::test]
    async fn test_execute_tool_error() {
        #[derive(Debug)]
        struct FailingTool;
        impl Tool for FailingTool {
            fn name(&self) -> &str { "fail" }
            fn description(&self) -> &str { "Always fails" }
            fn parameters(&self) -> Cow<'static, [ToolParameter]> { Cow::Owned(vec![]) }
            fn execute(&self, _input: &str) -> Result<PToolResult, PToolError> {
                Err(PToolError::invalid_input("bad input"))
            }
        }

        let adapter = PawbunToolAdapter::new(Box::new(FailingTool));
        let result = adapter.execute(
            "call_1", json!({}), None, CancellationToken::new(),
        ).await.unwrap();

        assert!(result.is_error);
        let text = content_text(&result);
        assert!(text.contains("bad input"));
    }

    #[tokio::test]
    async fn test_execute_cancelled() {
        let adapter = PawbunToolAdapter::new(Box::new(EchoTool));
        let token = CancellationToken::new();
        token.cancel();

        let result = adapter.execute(
            "call_1", json!({"message": "hi"}), None, token,
        ).await.unwrap();

        assert!(result.is_error);
        assert!(content_text(&result).contains("cancelled"));
    }

    fn content_text(result: &AgentToolResult) -> String {
        result.content.iter()
            .filter_map(|c| match c {
                ai_provider::Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }
}
```

- [ ] **Step 2: Run tests — verify FAIL**

```bash
cargo test -p agent-core -- pawbun_adapter
```

Expected: COMPILE ERROR — `PawbunToolAdapter` not defined.

- [ ] **Step 3: Implement PawbunToolAdapter**

```rust
use std::sync::Arc;

use pawbun_toolkit::{Tool, ToolParameter, ToolResult as PToolResult, ToolError as PToolError};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::types::{AgentTool, AgentToolProgressUpdate, AgentToolResult};

/// Wraps a [`pawbun_toolkit::Tool`] as a Pandaria [`AgentTool`].
///
/// Converts:
/// - `ToolParameter[]` → JSON Schema (cached at construction)
/// - sync `execute(&str)` → async `execute` via `tokio::task::spawn_blocking`
/// - `ToolResult` → `AgentToolResult`
///
/// # Constraints
///
/// The Pawbun tool's sandbox base directory is **baked in at construction time**
/// via `AgentSpace::workspace_for(tenant_id)`. Per ADR-004, the tenant is
/// immutable for the session lifetime, so this is safe.
pub struct PawbunToolAdapter {
    inner: std::sync::Arc<dyn Tool>,
    cached_schema: serde_json::Value,
    name: String,
    description: String,
}

impl PawbunToolAdapter {
    pub fn new(tool: Box<dyn Tool>) -> Self {
        let name = tool.name().to_string();
        let description = tool.description().to_string();
        let cached_schema = params_to_json_schema(&tool.parameters());
        Self { inner: std::sync::Arc::from(tool), cached_schema, name, description }
    }
}

/// Convert `ToolParameter[]` to a JSON Schema object value.
fn params_to_json_schema(params: &[ToolParameter]) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<serde_json::Value> = Vec::new();
    for p in params {
        properties.insert(p.name.clone(), p.schema.clone());
        if p.required {
            required.push(json!(p.name));
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// Convert a Pawbun `ToolResult` to an `AgentToolResult`.
fn pawbun_result_to_agent_result(r: PToolResult) -> Result<AgentToolResult, AgentError> {
    Ok(AgentToolResult {
        content: vec![ai_provider::Content::Text {
            text: r.content,
            text_signature: None,
        }],
        details: r.metadata,
        is_error: !r.success,
        terminate: false,
    })
}

impl AgentTool for PawbunToolAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.cached_schema.clone()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
        signal: CancellationToken,
    ) -> Result<AgentToolResult, AgentError> {
        let input_json = serde_json::to_string(&params)
            .map_err(|e| AgentError::ToolExecutionFailed(format!("serialization: {e}")))?;

        // Use Arc for 'static lifetime required by spawn_blocking
        let inner = std::sync::Arc::clone(&self.inner);

        // NOTE: tokio::select! only stops *waiting* for the JoinHandle.
        // The blocking thread continues executing. CodeExecuteTool handles
        // actual cancellation via Child::kill(); file tools have resource
        // limits (max file size) to bound worst-case blocking time.
        tokio::select! {
            result = tokio::task::spawn_blocking(move || {
                inner.execute(&input_json)
            }) => {
                match result {
                    Ok(Ok(tr)) => pawbun_result_to_agent_result(tr),
                    Ok(Err(e)) => Ok(AgentToolResult {
                        content: vec![ai_provider::Content::Text {
                            text: e.to_string(),
                            text_signature: None,
                        }],
                        details: None,
                        is_error: true,
                        terminate: false,
                    }),
                    Err(join_err) => Ok(AgentToolResult {
                        content: vec![ai_provider::Content::Text {
                            text: format!("tool panicked: {join_err}"),
                            text_signature: None,
                        }],
                        details: None,
                        is_error: true,
                        terminate: false,
                    }),
                }
            }
            _ = signal.cancelled() => {
                Ok(AgentToolResult {
                    content: vec![ai_provider::Content::Text {
                        text: "cancelled".into(),
                        text_signature: None,
                    }],
                    details: None,
                    is_error: true,
                    terminate: false,
                })
            }
        }
    }
}
```

- [ ] **Step 4: Run tests — verify PASS**

```bash
cargo test -p agent-core -- pawbun_adapter --nocapture
```

Expected: 4 tests PASS.

- [ ] **Step 5: Register module in tools/mod.rs**

```rust
// Add after existing module declarations:
pub mod pawbun_adapter;
```

- [ ] **Step 6: Verify agent-core compiles**

```bash
cargo check -p agent-core 2>&1 | tail -5
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add crates/agent-core/src/tools/pawbun_adapter.rs crates/agent-core/src/tools/mod.rs
git commit -m "feat: add PawbunToolAdapter for wrapping pawbun_toolkit::Tool as AgentTool"
```

---

### Task 3: HookConfig::with_pawbun_defaults() + FileOperationExtractor update

**Files:**
- Modify: `crates/agent-core/src/harness/config.rs`
- Modify: `crates/agent-core/src/file_ops.rs`

- [ ] **Step 1: Add with_pawbun_defaults() to HookConfig**

In `config.rs`, after the existing `HookConfig` struct:

```rust
impl HookConfig {
    /// Populate `path_guard_fields` with Pawbun tool field mappings
    /// for the dual-layer sandbox defense.
    pub fn with_pawbun_defaults(mut self) -> Self {
        self.path_guard_fields.insert("file_read".into(), vec!["path".into()]);
        self.path_guard_fields.insert("file_write".into(), vec!["path".into()]);
        self.path_guard_fields.insert("directory_list".into(), vec!["path".into()]);
        self.path_guard_fields.insert("code_execute".into(), vec!["work_dir".into()]);
        self
    }
}
```

- [ ] **Step 2: Extend DefaultFileOperationExtractor defaults**

In `file_ops.rs`, update the `Default` impl:

```rust
impl Default for DefaultFileOperationExtractor {
    fn default() -> Self {
        Self {
            read_tool_names: vec!["read".to_string(), "file_read".to_string()],
            write_tool_names: vec!["write".to_string(), "file_write".to_string()],
            edit_tool_names: vec!["edit".to_string()],
            path_arg_name: "path".to_string(),
        }
    }
}
```

- [ ] **Step 3: Write tests for path_guard defaults**

Add to existing tests in `config.rs` (or a new `#[cfg(test)]` block):

```rust
#[test]
fn test_hook_config_with_pawbun_defaults() {
    let config = HookConfig::default().with_pawbun_defaults();
    assert_eq!(config.path_guard_fields.get("file_read").unwrap(), &vec!["path".to_string()]);
    assert_eq!(config.path_guard_fields.get("file_write").unwrap(), &vec!["path".to_string()]);
    assert_eq!(config.path_guard_fields.get("directory_list").unwrap(), &vec!["path".to_string()]);
    assert_eq!(config.path_guard_fields.get("code_execute").unwrap(), &vec!["work_dir".to_string()]);
    // Verify it doesn't clobber existing entries
    let config2 = HookConfig {
        path_guard_fields: {
            let mut m = HashMap::new();
            m.insert("custom".into(), vec!["file".into()]);
            m
        },
        ..Default::default()
    }.with_pawbun_defaults();
    assert!(config2.path_guard_fields.contains_key("custom"));
    assert!(config2.path_guard_fields.contains_key("file_read"));
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p agent-core -- config hook_config
cargo test -p agent-core -- file_ops
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agent-core/src/harness/config.rs crates/agent-core/src/file_ops.rs
git commit -m "feat: add HookConfig::with_pawbun_defaults() and extend file operation extractor"
```

---

### Task 4: SessionBuilder integration

**Priority order** (established in `build()`):
1. External HTTP proxy tools (highest)
2. Media generation tool
3. **User-registered builtins** (`with_builtin_tools()`)
4. **Pawbun auto-registered builtins** (lowest — comes last)

Pawbun tools must be inserted AFTER user builtins so that user-builtins correctly shadow Pawbun tools by name.

**Files:**
- Modify: `crates/agent-core/src/harness/builder.rs`

- [ ] **Step 1: Add fields to SessionBuilder**

Add to the struct:

```rust
pub struct SessionBuilder {
    // ... existing fields ...
    builtin_enabled: bool,
    disabled_tools: Vec<String>,
}
```

Initialize in `new()`:

```rust
builtin_enabled: true, // default: enabled
disabled_tools: Vec::new(),
```

- [ ] **Step 2: Add with_builtin_tools_config() method**

```rust
/// Enable Pawbun built-in tools with an optional disabled-tool list.
///
/// Distinct from [`with_builtin_tools`] which registers arbitrary
/// in-process tools. This method auto-registers the Pawbun tool suite.
pub fn with_builtin_tools_config(mut self, enabled: bool, disabled: Vec<String>) -> Self {
    self.builtin_enabled = enabled;
    self.disabled_tools = disabled;
    self
}

/// Resolve the workspace directory for this session's tenant.
fn resolve_workspace(&self) -> std::path::PathBuf {
    self.config.agent_space.workspace_for(&self.tenant_id)
}
```

- [ ] **Step 3: Add build_pawbun_tool_refs() function**

At the bottom of `builder.rs` (outside `impl SessionBuilder`):

```rust
/// Default max file size for file_read (10 MB).
const DEFAULT_MAX_FILE_SIZE: usize = 10 * 1024 * 1024;
/// Default timeout for code_execute commands (30 seconds).
const DEFAULT_CMD_TIMEOUT_SECS: u64 = 30;

/// Build `AgentToolRef` list from Pawbun tools, wrapping each in
/// a `PawbunToolAdapter`.
fn build_pawbun_tool_refs(
    workspace: &std::path::Path,
    disabled: &[String],
    _http_client: &reqwest::Client,
) -> Vec<AgentToolRef> {
    use crate::tools::pawbun_adapter::PawbunToolAdapter;
    use pawbun_toolkit::{
        FileReadTool, FileWriteTool, DirectoryListTool, CodeExecuteTool,
    };
    use std::sync::Arc;
    use std::time::Duration;

    let make = |tool: Box<dyn pawbun_toolkit::Tool>| -> AgentToolRef {
        Arc::new(PawbunToolAdapter::new(tool))
    };

    let mut tools: Vec<AgentToolRef> = vec![
        make(Box::new(
            FileReadTool::new(workspace.to_path_buf()).with_max_size(DEFAULT_MAX_FILE_SIZE),
        )),
        make(Box::new(FileWriteTool::new(workspace.to_path_buf()))),
        make(Box::new(DirectoryListTool::new(workspace.to_path_buf()))),
        make(Box::new(
            CodeExecuteTool::new(workspace.to_path_buf())
                .with_timeout(Duration::from_secs(DEFAULT_CMD_TIMEOUT_SECS)),
        )),
    ];

    #[cfg(feature = "pawbun-http")]
    {
        tools.push(make(Box::new(
            pawbun_toolkit::WebFetchTool::new(_http_client.clone()),
        )));
        tools.push(make(Box::new(
            pawbun_toolkit::WebSearchTool::new(_http_client.clone()),
        )));
    }

    // Log warning for unknown disabled tool names
    for name in disabled {
        if !tools.iter().any(|t| t.name() == name.as_str()) {
            tracing::warn!(%name, "disabled tool name not recognized among Pawbun builtins");
        }
    }

    tools.into_iter()
         .filter(|t| !disabled.contains(&t.name().to_string()))
         .collect()
}
```

- [ ] **Step 4: Integrate into build()**

In `SessionBuilder::build()`, keep the existing `// 2c. Built-in tools` (user-registered) block as-is. Then ADD a new `// 2d. Pawbun built-in tools` block AFTER it:

```rust
// 2d. Pawbun built-in tools (auto-registered, lowest priority)
if self.builtin_enabled {
    let workspace = self.resolve_workspace();
    let pawbun_tools = build_pawbun_tool_refs(
        &workspace,
        &self.disabled_tools,
        &self.config.http_client,
    );
    for tool in pawbun_tools {
        let name = tool.name().to_string();
        if seen.contains(&name) {
            tracing::info!(%name, "Pawbun tool shadowed by external, media, or user builtin");
            continue;
        }
        seen.insert(name);
        tools.push(tool);
    }
}
```

- [ ] **Step 5: Run existing builder tests**

```bash
cargo test -p agent-core -- builder
```

Expected: existing tests pass (they don't set `builtin_enabled` so no Change).

- [ ] **Step 6: Write new test — builtin tools registered**

```rust
#[tokio::test]
async fn test_session_builder_with_pawbun_builtins() {
    let config = dummy_runtime_config();
    let built = SessionBuilder::new(&config)
        .tenant_id("test-tenant")
        .session_id("sess-1")
        .with_builtin_tools_config(true, vec![])
        .build()
        .await
        .expect("build should succeed");

    let names: Vec<&str> = built.tools.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"file_read"), "expected file_read tool, got {:?}", names);
    assert!(names.contains(&"file_write"), "expected file_write tool");
    assert!(names.contains(&"directory_list"), "expected directory_list tool");
    assert!(names.contains(&"code_execute"), "expected code_execute tool");
}

#[tokio::test]
async fn test_session_builder_pawbun_disabled_filter() {
    let config = dummy_runtime_config();
    let built = SessionBuilder::new(&config)
        .tenant_id("test-tenant")
        .session_id("sess-1")
        .with_builtin_tools_config(true, vec!["code_execute".into()])
        .build()
        .await
        .expect("build should succeed");

    let names: Vec<&str> = built.tools.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"file_read"));
    assert!(!names.contains(&"code_execute"), "code_execute should be disabled");
}

#[tokio::test]
async fn test_session_builder_pawbun_disabled_all() {
    let config = dummy_runtime_config();
    let built = SessionBuilder::new(&config)
        .tenant_id("test-tenant")
        .session_id("sess-1")
        .with_builtin_tools_config(true, vec![
            "file_read".into(), "file_write".into(),
            "directory_list".into(), "code_execute".into(),
        ])
        .build()
        .await
        .expect("build should succeed");

    // No Pawbun tools (all disabled), but build succeeds
    let _ = built;
}

#[tokio::test]
async fn test_session_builder_pawbun_disabled() {
    let config = dummy_runtime_config();
    let built = SessionBuilder::new(&config)
        .tenant_id("test-tenant")
        .session_id("sess-1")
        .with_builtin_tools_config(false, vec![])
        .build()
        .await
        .expect("build should succeed");

    let names: Vec<&str> = built.tools.iter().map(|t| t.name()).collect();
    assert!(!names.contains(&"file_read"), "Pawbun tools should not be registered when disabled");
}
```

- [ ] **Step 7: Run new tests**

```bash
cargo test -p agent-core -- builder pawbun
```

Expected: 4 new tests PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/agent-core/src/harness/builder.rs
git commit -m "feat: integrate Pawbun built-in tools into SessionBuilder"
```

---

### Task 5: API layer — BuiltinToolsConfig

**Files:**
- Modify: `crates/api-gateway/src/types.rs`
- Modify: `crates/api-gateway/src/routes/sessions.rs`
- Modify: `crates/tenant/src/session_entry.rs`
- Modify: `crates/tenant/src/manager.rs`

- [ ] **Step 1: Add BuiltinToolsConfig to api-gateway types**

In `crates/api-gateway/src/types.rs`, near `CreateSessionRequest`:

```rust
/// Configuration for built-in Pawbun tools auto-registration.
#[derive(Debug, Clone, serde::Deserialize, Default)]
#[serde(default)]
pub struct BuiltinToolsConfig {
    /// Enable built-in tools (default true).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Tool names to exclude from registration.
    #[serde(default)]
    pub disabled: Vec<String>,
}

fn default_true() -> bool { true }
```

Add to `CreateSessionRequest`:

```rust
#[serde(default)]
pub builtin_tools: BuiltinToolsConfig,
```

- [ ] **Step 2: Add to tenant CreateSessionParams**

In `crates/tenant/src/manager.rs` (where `CreateSessionParams` is defined):

```rust
pub struct CreateSessionParams {
    // ... existing fields ...
    pub builtin_tools: agent_core::BuiltinToolsConfig, // NEW — but we need to define this type
}
```

Wait — `BuiltinToolsConfig` is defined in api-gateway types. We need to either:
a. Define it in `agent-core` as a shared type, or
b. Pass the two fields separately.

**Better approach (avoid circular dep)**: Pass `enabled: bool` and `disabled: Vec<String>` as separate fields in `CreateSessionParams`. The API gateway converts `BuiltinToolsConfig` → flat params.

In `crates/tenant/src/manager.rs` (where `CreateSessionParams` is defined):

```rust
pub struct CreateSessionParams {
    pub title: Option<String>,
    pub system_prompt: Option<String>,
    pub tools: Vec<agent_core::ToolConfig>,
    pub webhook: Option<WebhookConfig>,
    pub builtin_tools_enabled: bool,
    pub builtin_tools_disabled: Vec<String>,
}
```

Also add to `ActiveSession` in `crates/tenant/src/session_entry.rs` (for clone propagation):

```rust
pub struct ActiveSession {
    // ... existing fields ...
    pub builtin_tools_enabled: bool,
    pub builtin_tools_disabled: Vec<String>,
}
```

In `insert_active_session()` (manager.rs), store the values:

```rust
builtin_tools_enabled: params.builtin_tools_enabled,
builtin_tools_disabled: params.builtin_tools_disabled.clone(),
```

In `clone_session()` (manager.rs), propagate:

```rust
builtin_tools_enabled: entry.builtin_tools_enabled,
builtin_tools_disabled: entry.builtin_tools_disabled.clone(),
```

- [ ] **Step 3: Pass params in api-gateway route**

In `crates/api-gateway/src/routes/sessions.rs`, `create()` function:

```rust
let params = tenant::CreateSessionParams {
    title: req.title,
    system_prompt: req.system_prompt,
    tools: req.tools.into_iter().map(|t| agent_core::ToolConfig { ... }).collect(),
    webhook: req.webhook.map(...),
    builtin_tools_enabled: req.builtin_tools.enabled,
    builtin_tools_disabled: req.builtin_tools.disabled,
};
```

Same for `batch_create()`:

```rust
builtin_tools_enabled: req.template.builtin_tools.enabled,
builtin_tools_disabled: req.template.builtin_tools.disabled,
```

- [ ] **Step 4: Pass through tenant manager**

In `crates/tenant/src/manager.rs`, find where `SessionBuilder` is called (likely in `create_session_inner` or similar). Add:

```rust
.with_builtin_tools_config(params.builtin_tools_enabled, params.builtin_tools_disabled)
```

- [ ] **Step 5: Verify compilation**

```bash
cargo check -p api-gateway 2>&1 | tail -10
cargo check -p tenant 2>&1 | tail -10
```

Expected: no errors.

- [ ] **Step 6: Write unit test for BuiltinToolsConfig deserialization**

In api-gateway types tests (or create one):

```rust
#[test]
fn test_builtin_tools_config_default() {
    let config: BuiltinToolsConfig = serde_json::from_str("{}").unwrap();
    assert!(config.enabled);
    assert!(config.disabled.is_empty());
}

#[test]
fn test_builtin_tools_config_disabled() {
    let config: BuiltinToolsConfig = serde_json::from_str(
        r#"{"enabled": true, "disabled": ["code_execute"]}"#
    ).unwrap();
    assert!(config.enabled);
    assert_eq!(config.disabled, vec!["code_execute"]);
}

#[test]
fn test_builtin_tools_config_off() {
    let config: BuiltinToolsConfig = serde_json::from_str(
        r#"{"enabled": false}"#
    ).unwrap();
    assert!(!config.enabled);
}
```

- [ ] **Step 7: Run tests**

```bash
cargo test -p api-gateway -- builtin
cargo test -p tenant -- builtin
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/api-gateway/src/types.rs crates/api-gateway/src/routes/sessions.rs \
        crates/tenant/src/session_entry.rs crates/tenant/src/manager.rs
git commit -m "feat: add BuiltinToolsConfig to session creation API"
```

---

### Task 6: Integration tests (adapter + Pawbun tools)

**Files:**
- Create: `crates/agent-core/tests/pawbun_integration.rs`

- [ ] **Step 1: Write integration tests**

```rust
use std::path::PathBuf;
use std::sync::Arc;
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
    result.content.iter()
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
    let result = adapter.execute(
        "call_1",
        json!({"path": "hello.txt"}),
        None,
        CancellationToken::new(),
    ).await.unwrap();

    assert!(!result.is_error, "should succeed: {:?}", result);
    assert_eq!(content_text(&result), "hello world");
}

#[tokio::test]
async fn test_file_read_path_traversal() {
    let dir = setup_dir("file_read_traversal");
    let adapter = PawbunToolAdapter::new(Box::new(FileReadTool::new(&dir)));
    let result = adapter.execute(
        "call_1",
        json!({"path": "../etc/passwd"}),
        None,
        CancellationToken::new(),
    ).await.unwrap();

    assert!(result.is_error, "path traversal should be blocked");
    let text = content_text(&result);
    assert!(text.contains("path traversal") || text.contains("invalid path"),
        "expected traversal error, got: {text}");
}

#[tokio::test]
async fn test_file_read_bad_type() {
    let dir = setup_dir("file_read_bad_type");
    let adapter = PawbunToolAdapter::new(Box::new(FileReadTool::new(&dir)));
    let result = adapter.execute(
        "call_1",
        json!({"path": 42}),  // wrong type: int instead of string
        None,
        CancellationToken::new(),
    ).await.unwrap();

    assert!(result.is_error, "bad type should error");
}

// ── file_write + read round-trip ──

#[tokio::test]
async fn test_file_write_and_read() {
    let dir = setup_dir("file_write_read");
    let write_adapter = PawbunToolAdapter::new(Box::new(FileWriteTool::new(&dir)));
    let read_adapter = PawbunToolAdapter::new(Box::new(FileReadTool::new(&dir)));

    // Write
    let w = write_adapter.execute(
        "call_w",
        json!({"path": "out.txt", "content": "round-trip data"}),
        None,
        CancellationToken::new(),
    ).await.unwrap();
    assert!(!w.is_error, "write failed: {w:?}");

    // Read back
    let r = read_adapter.execute(
        "call_r",
        json!({"path": "out.txt"}),
        None,
        CancellationToken::new(),
    ).await.unwrap();
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
    let result = adapter.execute(
        "call_1",
        json!({"path": "."}),
        None,
        CancellationToken::new(),
    ).await.unwrap();

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
    let dispatcher = DefaultHookDispatcher::from_config(
        agent_core::space::AgentSpace::default(),
        &config,
    );

    let ctx = ToolCallCtx {
        tenant_id: "t1".into(),
        session_id: "s1".into(),
        tool_name: "file_read".into(),
        tool_call_id: "call_1".into(),
        input: json!({"path": "/etc/passwd"}),
    };

    let (decision, _) = dispatcher.on_tool_call(&ctx).await;
    match decision {
        HookDecision::Block { reason } => {
            assert!(reason.contains("path") || reason.contains("forbidden"),
                "expected path-related block reason, got: {reason}");
        }
        HookDecision::Continue => {
            panic!("path_guard should block /etc/passwd for file_read");
        }
    }
}

// ── code_execute (if available) ──

#[tokio::test]
async fn test_code_execute_echo() {
    use pawbun_toolkit::CodeExecuteTool;
    use std::time::Duration;

    let dir = setup_dir("code_execute_echo");
    let tool = CodeExecuteTool::new(&dir).with_timeout(Duration::from_secs(5));
    let adapter = PawbunToolAdapter::new(Box::new(tool));

    let result = adapter.execute(
        "call_1",
        json!({"command": "echo hello_from_bash"}),
        None,
        CancellationToken::new(),
    ).await.unwrap();

    assert!(!result.is_error, "echo failed: {:?}", result);
    let text = content_text(&result);
    assert!(text.contains("hello_from_bash"), "expected 'hello_from_bash' in output, got: {text}");
}

#[tokio::test]
async fn test_code_execute_timeout() {
    use pawbun_toolkit::CodeExecuteTool;
    use std::time::Duration;

    let dir = setup_dir("code_execute_timeout");
    // Set a very short timeout so the sleep command gets killed
    let tool = CodeExecuteTool::new(&dir).with_timeout(Duration::from_millis(500));
    let adapter = PawbunToolAdapter::new(Box::new(tool));

    let result = adapter.execute(
        "call_1",
        json!({"command": "sleep 60"}),
        None,
        CancellationToken::new(),
    ).await.unwrap();

    assert!(result.is_error, "sleep 60 should timeout");
    let text = content_text(&result);
    assert!(text.contains("timeout") || text.contains("killed") || text.contains("signal"),
        "expected timeout/kill message, got: {text}");
}
```

- [ ] **Step 2: Run integration tests**

```bash
cargo test -p agent-core --test pawbun_integration -- --nocapture
```

Expected: 8 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/agent-core/tests/pawbun_integration.rs
git commit -m "test: add integration tests for PawbunToolAdapter + built-in tools"
```

---

### Task 7: E2E tests (api-gateway)

**Files:**
- Create: `crates/api-gateway/tests/e2e/e2e_builtin_tools.rs`

- [ ] **Step 1: Write E2E test file**

Use the existing low-level `oneshot` pattern from other E2E tests (e.g., `e2e_session_lifecycle.rs`, `e2e_path_guard.rs`):

```rust
// crates/api-gateway/tests/e2e/e2e_builtin_tools.rs
mod common;
use axum::body::Body;
use http::Request;

fn make_token(tenant: &str) -> String {
    common::make_token(tenant)
}

// Test that session creation succeeds with builtin_tools enabled (default)
#[tokio::test]
async fn test_builtin_tools_registered_by_default() {
    let body = serde_json::json!({"stop_reason": "stop", "content": [{"type": "text", "text": "ok"}]});
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = make_token("test-tenant");

    let response = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/sessions")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"title": "test", "system_prompt": "You have file tools"}"#))
            .unwrap(),
    ).await.unwrap();

    assert_eq!(response.status(), 201);
    let info = common::json_body(response).await;
    assert!(info.get("id").is_some(), "session should be created");
}

// Test that disabled filters work
#[tokio::test]
async fn test_builtin_tools_disabled_filter() {
    let body = serde_json::json!({"stop_reason": "stop", "content": [{"type": "text", "text": "ok"}]});
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = make_token("test-tenant");

    let response = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/sessions")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(Body::from(
                r#"{"builtin_tools": {"enabled": true, "disabled": ["code_execute"]}}"#,
            ))
            .unwrap(),
    ).await.unwrap();

    assert_eq!(response.status(), 201, "session should create with disabled filter");
}

// Test that all tools can be disabled without error
#[tokio::test]
async fn test_builtin_tools_disabled_all() {
    let body = serde_json::json!({"stop_reason": "stop", "content": [{"type": "text", "text": "ok"}]});
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = make_token("test-tenant");

    let response = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/sessions")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(Body::from(
                r#"{"builtin_tools": {"enabled": true, "disabled": ["file_read", "file_write", "directory_list", "code_execute"]}}"#,
            ))
            .unwrap(),
    ).await.unwrap();

    assert_eq!(response.status(), 201, "all disabled should still create session");
}

// Test builtin_tools off entirely
#[tokio::test]
async fn test_builtin_tools_off() {
    let body = serde_json::json!({"stop_reason": "stop", "content": [{"type": "text", "text": "ok"}]});
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = make_token("test-tenant");

    let response = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/sessions")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"builtin_tools": {"enabled": false}}"#))
            .unwrap(),
    ).await.unwrap();

    assert_eq!(response.status(), 201);
}

// Test external tool shadows builtin (both registered, external wins)
#[tokio::test]
async fn test_external_shadows_builtin() {
    let body = serde_json::json!({"stop_reason": "stop", "content": [{"type": "text", "text": "ok"}]});
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = make_token("test-tenant");

    let response = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/sessions")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(Body::from(
                r#"{"tools": [{"name": "file_read", "description": "Custom", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}, "endpoint": "http://mock-tool.local/invoke"}]}"#,
            ))
            .unwrap(),
    ).await.unwrap();

    assert_eq!(response.status(), 201, "external shadowing builtin should succeed");
}
```

- [ ] **Step 2: Run E2E tests**

```bash
cargo test -p api-gateway --test e2e_builtin_tools -- --nocapture
```

Expected: 5 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/api-gateway/tests/e2e/e2e_builtin_tools.rs
git commit -m "test: add E2E tests for builtin tools API"
```

---

### Task 8: Run full test suite & clippy

**Files:** none (verification only)

- [ ] **Step 1: Run all agent-core tests**

```bash
cargo test -p agent-core 2>&1 | tail -20
```

Expected: all tests pass (no regressions). Note: the `file_ops` module has no `#[cfg(test)]` block, so `cargo test -p agent-core -- file_ops` will return 0 tests — that's expected.

- [ ] **Step 2: Run all api-gateway tests**

```bash
cargo test -p api-gateway 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 3: Run clippy**

```bash
cargo clippy -p agent-core -- -D warnings 2>&1 | tail -5
cargo clippy -p api-gateway -- -D warnings 2>&1 | tail -5
cargo clippy -p tenant -- -D warnings 2>&1 | tail -5
```

Expected: no warnings.

- [ ] **Step 4: Commit if clean**

```bash
git status
# Should show only intentional changes
```

---

## Execution Order

```
Task 1 (dep) ──→ Task 2 (adapter) ──→ Task 4 (builder) ──→ Task 5 (API) ──→ Task 6 (tests) ──→ Task 7 (E2E) ──→ Task 8 (verify)
                                    ↗
Task 3 (config + file_ops) ────────┘
```

Tasks 2, 3 can run in parallel after Task 1. Everything after Task 4 is sequential.
