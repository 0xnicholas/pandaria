# 内置工具集集成：Pawbun → Pandaria

**日期**: 2026-06-13  
**状态**: 设计中  
**关联项目**: [Pawbun](https://github.com/0xnicholas/pawbun)

---

## 1. 问题陈述

当前 Pandaria 的 `AgentTool` trait 是工具执行的标准协议，但框架不内置任何基础工具（bash、read_file、write_file 等）。所有工具需外部提供，目前唯一的路径是通过 `CreateSessionRequest.tools: Vec<ToolConfig>` 注册 HTTP proxy 工具——每个 tool call 都是一个完整的 HTTP 往返（网络延迟 + JSON 序列化 + SSRF 检查 + 超时管理），开销很高。

### 已有资产

[Pawbun](https://github.com/0xnicholas/pawbun) 是一个独立的 Rust 工具套件 crate，已实现了一套 `Tool` trait 和以下工具：

| 工具 | 状态 | feature gate |
|------|------|-------------|
| `file_read` | 完整实现（路径沙箱、文件大小限制） | — |
| `file_write` | 完整实现（自动建目录、TOCTOU 检测） | — |
| `directory_list` | 完整实现 | — |
| `web_fetch` | 完整实现 | `http` |
| `web_search` | 完整实现 | `http` |
| `csv_query` | 完整实现 | `csv` |
| `json_query` | 完整实现 | `jsonpath` |
| `code_execute` | 占位接口，需完整实现 | — |
| `vision` | 占位接口 | — |

Pawbun 的 `Tool` trait 与 Pandaria 的 `AgentTool` trait 是两套不兼容的协议——需要适配层。

### Pawbun `Tool` trait 签名（已确认）

```rust
// crates/pawbun-toolkit/src/tool.rs
pub trait Tool: Debug + Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Cow<'static, [ToolParameter]>;
    fn execute(&self, input: &str) -> Result<ToolResult, ToolError>;
}
```

`execute` 接收 `&str`（原始 JSON 字符串），需要适配层将 `serde_json::Value` 序列化为字符串后传入。

---

## 2. 设计决策

### 2.1 集成模式：适配器

Pawbun 保持为独立 crate，不依赖 Pandaria。适配器放在 `agent-core` 内，负责 `pawbun_toolkit::Tool` → `agent_core::AgentTool` 的协议转换。

**理由**：
- Pawbun 可被其他项目独立使用（类似 CrewAI Tools 的定位）
- 适配器是单向映射，复杂度可控
- 不要求 Pawbun 感知 Pandaria 的存在

### 2.2 工具范围：全量内置

所有 Pawbun 工具通过适配器注册为内置工具集。`code_execute`（bash）在 Pawbun 侧通过 `std::process::Command` 完整实现。

### 2.3 bash 执行：subprocess 直接执行

```rust
std::process::Command::new("bash")
    .arg("-c")
    .arg(command)
    .current_dir(work_dir)
    .output()
```

安全边界：
- Pawbun 层：`work_dir` 限定在沙箱内 + 超时 kill + 命令白名单
- Pandaria 层：`path_guard` hook 拦截文件访问 + `tool_guard` hook 可拒绝执行

### 2.4 不可变约束

Pawbun 工具的 `base_dir`（工作空间路径）在 `SessionActor` 构造时通过 `AgentSpace::workspace_for(tenant_id)` 计算并固化。依 ADR-004（session 间无共享可变状态），session 的 `tenant_id` 是不可变的——因此沙箱路径在 session 生命周期内不会改变。此约束需在适配器文档中明确标注。

---

## 3. 架构

```
┌────────────────────────────────────────────────────────┐
│  api-gateway                                           │
│    CreateSessionRequest {                              │
│      tools: Vec<ToolConfig>,        // 外部 HTTP proxy │
│      builtin_tools: BuiltinToolsConfig,  // 新增       │
│    }                                                   │
│    SessionBuilder                                      │
│      .with_external_tools(tools)                       │
│      .with_builtin_tools(enabled, disabled)  // 新增   │
├────────────────────────────────────────────────────────┤
│  agent-core                                            │
│  ├── tools/pawbun_adapter.rs          // 新增          │
│  │     PawbunToolAdapter: AgentTool                   │
│  │       ├── ToolParameter[] → JSON Schema (缓存)      │
│  │       ├── sync execute → spawn_blocking             │
│  │       ├── CancellationToken → 取消                  │
│  │       └── ToolResult → AgentToolResult              │
│  ├── tools/http_proxy.rs             // 已有           │
│  └── harness/builder.rs              // 修改           │
│        build_pawbun_tool_refs()      // 新增           │
├────────────────────────────────────────────────────────┤
│  pawbun-toolkit (外部 crate)                           │
│  ├── FileReadTool, FileWriteTool, DirectoryListTool    │
│  ├── WebFetchTool, WebSearchTool                       │
│  ├── CodeExecuteTool  // 需完整实现                     │
│  └── Tool trait + ToolKit                              │
└────────────────────────────────────────────────────────┘
```

---

## 4. 核心组件

### 4.1 `PawbunToolAdapter`

位置：`crates/agent-core/src/tools/pawbun_adapter.rs`

```rust
pub struct PawbunToolAdapter {
    inner: Box<dyn pawbun_toolkit::Tool>,
    cached_schema: serde_json::Value,
}

impl PawbunToolAdapter {
    pub fn new(tool: Box<dyn pawbun_toolkit::Tool>) -> Self {
        let cached_schema = params_to_json_schema(&tool.parameters());
        Self { inner: tool, cached_schema }
    }
}
```

#### 4.1.1 Schema 转换

`ToolParameter[]` → JSON Schema (构建时一次性计算，缓存)：

```rust
fn params_to_json_schema(params: &[ToolParameter]) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for p in params {
        properties.insert(p.name.clone(), p.schema.clone());
        if p.required { required.push(p.name.clone()); }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}
```

`ToolParameter.schema` 字段（`serde_json::Value`）是 JSON Schema 片段，直接复用——不做转换。

#### 4.1.2 执行桥接

```rust
async fn execute(&self, tool_call_id, params, on_progress, signal) {
    let input_json = serde_json::to_string(&params)?;

    tokio::select! {
        result = tokio::task::spawn_blocking(move || {
            self.inner.execute(&input_json)
        }) => {
            match result {
                Ok(Ok(tr)) => pawbun_result_to_agent_result(tr),
                Ok(Err(e)) => Err(AgentError::ToolExecutionFailed(e.to_string())),
                Err(je)  => Err(AgentError::ToolExecutionFailed(format!("panic: {je}"))),
            }
        }
        _ = signal.cancelled() => {
            Ok(AgentToolResult {
                content: vec![Content::Text { text: "cancelled".into(), .. }],
                is_error: true,
                ..
            })
        }
    }
}
```

**局限**：`spawn_blocking` 不中断正在执行的同步代码。如果 bash 子进程卡住，需等到它自己退出。后续可通过向子进程发 SIGTERM 改进。

#### 4.1.3 结果映射

| Pawbun | Pandaria |
|--------|----------|
| `ToolResult { success: true, content, metadata }` | `AgentToolResult { content: vec![Text(content)], details: metadata, is_error: false }` |
| `ToolResult { success: false, content }` | `AgentToolResult { content: vec![Text(content)], is_error: true }` |
| `ToolError` | `AgentError::ToolExecutionFailed(msg)` — 作为工具错误返回，**不中断 session** |
| `ToolError::Timeout` | 在适配层由 `CancellationToken` 处理，不到达 `execute` |

### 4.2 CodeExecuteTool

位置：`crates/pawbun-toolkit/src/tools/code_execute.rs`（覆盖现有占位）

```rust
pub struct CodeExecuteTool {
    pub work_dir: Option<PathBuf>,
    pub timeout: Duration,
    pub allowed_commands: Vec<String>,
}
```

输入：`{"command": "ls -la", "work_dir": "optional/subdir"}`

执行流程：
1. 解析 `command` 字符串
2. 安全校验：白名单命令（若配置）、禁止模式检测
3. 在 `spawn_blocking` 中执行 `bash -c "{command}"`
4. 捕获 stdout + stderr，合并输出
5. 返回 `{exit_code, stdout, stderr, elapsed_ms}`

超时策略：`std::process::Child::wait_timeout(timeout)` → 超时则 `kill()`。

### 4.3 Builder 集成

修改 `crates/agent-core/src/harness/builder.rs`：

```rust
pub struct SessionBuilder {
    // ... 已有字段 ...
    builtin_enabled: bool,
    disabled_tools: Vec<String>,
}

impl SessionBuilder {
    pub fn with_builtin_tools_config(mut self, enabled: bool, disabled: Vec<String>) -> Self {
        self.builtin_enabled = enabled;
        self.disabled_tools = disabled;
        self
    }

    fn resolve_workspace(&self) -> PathBuf {
        self.config.agent_space.workspace_for(&self.tenant_id)
    }
}

fn build_pawbun_tool_refs(
    workspace: &Path,
    disabled: &[String],
    http_client: &reqwest::Client,
) -> Vec<AgentToolRef> {
    let mut tools: Vec<AgentToolRef> = vec![
        adapt(FileReadTool::new(workspace).with_max_size(DEFAULT_MAX_FILE_SIZE)),
        adapt(FileWriteTool::new(workspace)),
        adapt(DirectoryListTool::new(workspace)),
        adapt(CodeExecuteTool::new(workspace)
            .with_timeout(Duration::from_secs(DEFAULT_CMD_TIMEOUT_SECS))),
    ];
    // Web tools via Pawbun's `http` feature (opt-in via agent-core's `pawbun-http` feature)
    #[cfg(feature = "pawbun-http")]
    {
        tools.push(adapt(WebFetchTool::new(http_client.clone())));
        tools.push(adapt(WebSearchTool::new(http_client.clone())));
    }
    tools.into_iter()
         .filter(|t| !disabled.contains(&t.name().to_string()))
         .collect()
}
```

**工作空间解析**：`build_pawbun_tool_refs` 接收 `workspace` 参数，该参数由 `SessionBuilder::resolve_workspace()` 通过 `AgentSpace::workspace_for(tenant_id)` 计算，**不是任意路径**。

**Feature gate**：`agent-core` 新增 `pawbun-http` feature，对应转发 Pawbun 的 `http` feature（启用 `WebFetchTool`/`WebSearchTool`）。当 Pawbun 作为 git 依赖时，feature 通过 cargo 的 feature 传递机制转发。

**资源限制常量**：
- `DEFAULT_MAX_FILE_SIZE`: 10 MB（避免 `file_read` 占用过多内存）
- `DEFAULT_CMD_TIMEOUT_SECS`: 30 秒

**优先级**：external > media generation > builtin（已有逻辑不变）。同名冲突时内置被 shadow。

---

## 5. API 变更

### 5.1 `CreateSessionRequest`

新增 `builtin_tools` 字段：

```rust
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub title: Option<String>,
    pub system_prompt: Option<String>,
    pub tools: Vec<ToolConfig>,
    pub webhook: Option<WebhookConfig>,

    #[serde(default)]
    pub builtin_tools: BuiltinToolsConfig,
}

#[derive(Debug, Deserialize, Default)]
pub struct BuiltinToolsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub disabled: Vec<String>,
}
```

### 5.2 行为

- `builtin_tools.enabled = true`（默认）：所有 Pawbun 工具自动注册
- `builtin_tools.disabled = ["code_execute"]`：排除特定工具
- 同名外部工具（`tools` 中的 `ToolConfig`）优先覆盖内置

---

## 6. 安全模型

双层防御：

| 层级 | 组件 | 措施 |
|------|------|------|
| Pawbun | `resolve_sandbox_path()` | `base_dir` 沙箱 + 路径遍历检测 |
| Pawbun | `CodeExecuteTool` | `work_dir` 限定 + 命令白名单 + 超时 kill |
| Pawbun | `FileWriteTool` | TOCTOU 二次校验（canonicalize 后再次检查前缀） |
| Pandaria | `path_guard` hook | `on_tool_call` 阶段拦截文件路径参数 |
| Pandaria | `tool_guard` hook | `on_tool_call` 阶段拒绝指定工具执行 |

### 6.1 PathGuard 配置

`DefaultHookDispatcher` 的 path_guard 通过 `HookConfig.path_guard_fields` 配置（`HashMap<String, Vec<String>>`，工具名 → 路径字段名列表）。Pawbun 工具必须显式添加映射，否则 path_guard 不会拦截：

```rust
// HookConfig 中新增辅助方法
impl HookConfig {
    pub fn with_pawbun_defaults(mut self) -> Self {
        self.path_guard_fields.insert("file_read".into(), vec!["path".into()]);
        self.path_guard_fields.insert("file_write".into(), vec!["path".into()]);
        self.path_guard_fields.insert("directory_list".into(), vec!["path".into()]);
        self.path_guard_fields.insert("code_execute".into(), vec!["work_dir".into()]);
        self
    }
}
```

**注意区分**：`DefaultFileOperationExtractor`（`file_ops.rs`）仅供 Compactor 使用，与 PathGuard hook 是两套独立系统。需同步更新 Compactor 的工具名：

```rust
// file_ops.rs Default impl 中
read_tool_names: vec!["read".into(), "file_read".into()],
write_tool_names: vec!["write".into(), "file_write".into()],
edit_tool_names: vec!["edit".into()],
```

### 6.2 path_guard_scan_unknown 注意事项

`HookConfig.path_guard_scan_unknown`（默认 `false`）开启后会对所有未知工具的参数进行启发式路径扫描。Pawbun 的 `web_fetch`/`web_search` 工具的 URL 参数若被误识别为路径，会导致误拦截。**推荐保持默认 `false`**，转为所有文件操作工具显式配置 `path_guard_fields`。

---

## 7. 测试策略

### 7.1 单元测试（agent-core）

| 测试 | 验证点 |
|------|--------|
| Schema 转换 | `ToolParameter[]` → JSON Schema 结构正确（properties、required） |
| Schema 缓存 | 多次调用 `parameters()` 返回同一引用 |
| 结果映射 | `ToolResult { success: true }` → `AgentToolResult { is_error: false }` |
| 错误映射 | `ToolError::NotFound` → `AgentError::ToolExecutionFailed` |
| 取消 | `CancellationToken` cancel 后返回 `is_error: true` |

### 7.2 集成测试（agent-core，使用临时目录）

| 测试 | 验证点 |
|------|--------|
| `file_read` 成功 | 沙箱内文件读取返回正确内容 |
| `file_read` 路径遍历 | 返回 `is_error: true` |
| `file_read` 参数类型错误 | `{"path": 42}` 返回 `is_error: true`（Pawbun 的 JSON 解析报错） |
| `file_write` + `file_read` 往返 | 写入后可读回 |
| `file_write` TOCTOU | 符号链接逃逸后被二次校验拦截 |
| `directory_list` | 返回 JSON 数组，目录优先排序 |
| `code_execute` 简单命令 | `echo hello` → stdout |
| `code_execute` 超时 | `sleep 60` 被 kill，返回错误 |
| `code_execute` shell 注入尝试 | `ls; rm -rf /` → 被 Pawbun 校验拒绝 |
| PathGuard + Pawbun | `path_guard` 配置 `file_read` 的 `path` 字段，验证 `/etc/passwd` 被拦截 |

### 7.3 E2E 测试（api-gateway + testcontainers）

| 测试 | 验证点 |
|------|--------|
| 内置工具启用 | LLM tool call `file_read` → 进程内执行成功 |
| 内置工具被覆盖 | 外部 HTTP proxy 同名工具优先 |
| `disabled` 过滤 | `code_execute` 不在 tools 列表中 |
| `disabled` 未知工具名 | 打印 warning log，不影响 session 创建 |
| `disabled` 全部工具 | 结果为零工具，不报错 |
| `builtin_tools: {enabled: false}` | 无内置工具注册 |
| 并发：内置 + 外部工具共存 | 两种工具均可被 LLM 调用 |

---

## 8. 实现计划

| Phase | 内容 | 依赖 | 预计文件 |
|-------|------|------|---------|
| **1** | `pawbun_adapter.rs` — 适配器 + 单元测试 | Pawbun crate (git dep) | 1 新文件 |
| **2** | `CodeExecuteTool` 完整实现（Pawbun 仓库，独立 PR） | 无 | 1 文件覆盖 |
| **3** | `HookConfig::with_pawbun_defaults()` + `DefaultFileOperationExtractor` 更新 | 无 | `config.rs`, `file_ops.rs` 修改 |
| **4** | `SessionBuilder` 集成 — `BuiltinToolsConfig` → 自动注册 | Phase 1, 3 | `builder.rs` 修改 |
| **5** | API 层 — `CreateSessionRequest.builtin_tools` + route 适配 | Phase 4 | `types.rs`, `sessions.rs` 修改 |
| **6** | E2E 测试 | Phase 5 | 1 新测试文件 |

Phases 1-3 可并行，4-6 必须顺序。Phase 2 是 Pawbun 仓库的独立修改。Pawbun 作为 git 依赖引入（`agent-core/Cargo.toml`）：

```toml
[dependencies]
pawbun-toolkit = { git = "https://github.com/0xnicholas/pawbun", branch = "main" }
```

---

## 9. 风险与局限

| 风险 | 缓解 |
|------|------|
| `spawn_blocking` 不中断正在执行的同步代码 | `CodeExecuteTool` 通过 `Child::kill()` 协作取消。文件操作工具通过资源限制（`DEFAULT_MAX_FILE_SIZE` 10MB）防止极端情况。长目录列表的风险通过 `DirectoryListTool` 的超时机制缓解。后续可引入全局 `spawn_blocking` 超时包装器 |
| `spawn_blocking` 线程池饱和 | 默认 512 线程；若大量工具同时卡住，新任务排队。通过资源限制 + 超时减少卡住概率 |
| Pawbun 工具 `&str` 输入 vs Pandaria `serde_json::Value` 输入 | 适配层序列化一次，开销可接受（JSON 序列化极快） |
| `path_guard` 字段匹配依赖参数名 | 通过 `HookConfig::with_pawbun_defaults()` 显式配置，不依赖隐式匹配 |
| `tool_guard` 的 `denied_tools` 默认空 | 生产环境需运维配置 `PANDARIA_DENIED_TOOLS=code_execute` |
| `path_guard_scan_unknown` 误拦截 URL | 保持默认 `false`；若需要开启，为 `web_fetch`/`web_search` 添加空的 `path_guard_fields` 条目以跳过扫描 |
| Pawbun 作为外部 git 依赖的版本一致性 | 锁定 `Cargo.lock`；Phase 2 的 `CodeExecuteTool` 需在适配器集成前合并到 Pawbun main 分支 |
