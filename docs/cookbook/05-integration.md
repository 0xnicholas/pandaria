# 第五章：集成指南

> **目标读者**：想把 Pandaria 与卫星项目接起来的开发者。  
> **前提**：已读第四章（生态项目概览），了解各项目的职责。

---

## 5.1 Emerald — 记忆系统

### 5.1.1 架构

```
Pandaria SessionActor
      │
      ├── on_before_agent_start()
      │     └── MemoryHook ──► EmeraldMemoryStore::recall()
      │                              │
      │                              ▼
      │                     POST /v1/search  (Emerald)
      │                              │
      │                              ▼
      │                     返回用户画像/记忆 → 注入 prompt
      │
      └── on_turn_end()
            └── MemoryHook ──► EmeraldMemoryStore::remember()
                                     │
                                     ▼
                            POST /v1/memories  (Emerald)
                                     │
                                     ▼
                            fire-and-forget（不阻塞 agent loop）
```

### 5.1.2 关键设计原则

1. **Emerald 不感知 Pandaria**：所有集成逻辑在 `EmeraldMemoryStore` adapter 中
   - `tenant_id` → Emerald 的 `entity_id`
   - `session_id` / `turn_index` / `model` → Emerald 的 `metadata`（完整透传，Emerald 不解析）
2. **`content_type = "conversation"`**：Pandaria 发送 Markdown 格式对话 transcript
3. **`recall` 同步**（需要结果注入 prompt），**`remember` 异步**（fire-and-forget + 批量缓冲）

### 5.1.3 接口映射

| Pandaria MemoryStore 方法 | Emerald API | 调用时机 |
|---|---|---|
| `remember(ctx, content)` | `POST /v1/memories` | Turn 结束时（异步） |
| `recall(ctx, query)` | `POST /v1/search` | Agent 启动前（同步，含超时保护） |
| `forget_session(session_id)` | `DELETE /v1/sessions/{id}` | Session 关闭时 |

### 5.1.4 配置

```rust
// 在 Pandaria 的 session 配置中
EmeraldMemoryStore::new(
    "http://emerald:8000".into(),  // Emerald base URL
    reqwest::Client::new(),
)
.with_recall_timeout(Duration::from_millis(500))
.with_prewarm(true);  // session 创建时后台拉取用户画像
```

### 5.1.5 对话格式化

Pandaria 将多轮对话转为 Markdown transcript 发送给 Emerald：

```markdown
## Session: {session_id} | Turn {n}

**User**: 帮我研究一下 Rust 的 async trait

**Assistant**: Rust 的 async trait 在 1.75 版本中稳定了...

**Tool Call**: web_search
**Tool Result**: [搜索结果...]
```

Emerald 的提取管线自动从中抽取事实、建立关系、更新知识图谱。

### 5.1.6 故障处理

| 场景 | 行为 |
|------|------|
| Emerald 不可达（remember） | 后台 worker 打 warn log，不影响 agent loop |
| Emerald 不可达（recall） | 超时后返回空结果（`Vec::new()`），agent 无记忆上下文但正常启动 |
| Emerald 返回 4xx/5xx | 同上 |
| prewarm 失败 | 不影响 session 创建，首次 recall 走同步路径 |

---

## 5.2 Pawbun — 工具系统

### 5.2.1 架构

```
Pandaria ToolExecutor
      │
      ├── 内置工具（AgentTool trait 直接实现）
      │
      └── PawbunToolAdapter
            │
            ├── PawbunToolVariant::Sync(tool)  ← spawn_blocking 执行
            │     └─ FileReadTool, FileWriteTool, DirectoryListTool, ...
            │
            └── PawbunToolVariant::Async(tool) ← 原生 async 执行
                  └─ WebFetchTool, WebSearchTool, ...
```

### 5.2.2 适配层设计

Pawbun 有两个 trait——`Tool`（同步 `execute`）和 `AsyncTool`（异步 `execute_async`）。适配层用一个枚举统一处理：

```rust
pub struct PawbunToolAdapter {
    inner: PawbunToolVariant,
    tenant_id: String,
    session_id: String,
}

enum PawbunToolVariant {
    /// 通过 spawn_blocking 在 tokio 阻塞线程池执行
    Sync(Arc<dyn pawbun_toolkit::Tool>),
    /// 原生 async 执行
    Async(Arc<dyn pawbun_toolkit::AsyncTool>),
}

impl AgentTool for PawbunToolAdapter {
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<AgentToolResult, AgentToolError> {
        match &self.inner {
            PawbunToolVariant::Sync(tool) => {
                tokio::task::spawn_blocking(move || tool.execute(input))
                    .await??  // JoinError + ToolError
            }
            PawbunToolVariant::Async(tool) => {
                tool.execute_async(input).await?
            }
        }
    }
}
```

### 5.2.3 添加 Cargo 依赖

```toml
# agent-core/Cargo.toml
[dependencies]
pawbun-toolkit = "0.2"
pawbun-files = "0.2"     # 可选：多模态文件处理
pawbun-mcp-core = "0.2"  # 可选：MCP 协议支持
```

### 5.2.4 在 SessionActor 中使用

```rust
// 创建 session 时注入 Pawbun 工具
let tools: Vec<Arc<dyn AgentTool>> = vec![
    Arc::new(PawbunToolAdapter::new_sync(
        Arc::new(FileReadTool::new(workspace_root)),
        tenant_id.clone(),
        session_id.clone(),
    )),
    Arc::new(PawbunToolAdapter::new_async(
        Arc::new(WebSearchTool::new(search_api_url)),
        tenant_id.clone(),
        session_id.clone(),
    )),
];

let session = SessionActor::new(
    tenant_id, session_id,
    tools, hook_dispatcher, provider, ...
);
```

### 5.2.5 安全集成

Pawbun 内置工具自带安全防护，与 Pandaria 的 hook 系统叠加：

| 安全层 | 实现位置 | 作用 |
|--------|---------|------|
| **PathGuard** (Pandaria hook) | `DefaultHookDispatcher` | 工具执行前校验文件路径在 workspace 内 |
| **沙箱路径** (Pawbun) | `FileReadTool` / `FileWriteTool` | `canonicalize` + 前缀校验 |
| **SSRF 防护** (Pawbun) | `WebFetchTool` / MCP 客户端 | 禁止内网/本地地址 |
| **ToolGuard** (Pandaria hook) | `DefaultHookDispatcher` | allow/deny 名单控制工具可用性 |

**双重保护**：Pandaria hook 在工具执行前做第一层拦截（阻断型），Pawbun 内部做第二层校验。即使 Pawbun 工具被直接调用（绕过 Pandaia hook），自身安全防护仍然生效。

### 5.2.6 MCP 客户端接入

```rust
use pawbun_toolkit::mcp::StdioTransport;
use pawbun_toolkit::DynamicTool;

// 连接外部 MCP 服务器
let transport = StdioTransport::new("python mcp_server.py");
let dynamic_tool = DynamicTool::connect(transport).await?;

// 包装为 Pandaria AgentTool
let adapter = PawbunToolAdapter::new_async(
    Arc::new(dynamic_tool),
    tenant_id.clone(),
    session_id.clone(),
);
```

---

## 5.3 Tavern — 编排框架

### 5.3.1 架构

```
Tavern Server (axum)
      │
      ├── POST /workflows/:id/run
      │     │
      │     └── tavern-comp Workflow Engine
      │           │
      │           ├── Step 1: "research"
      │           │     └── PandariaRuntime::send_message(session_id, task)
      │           │           │
      │           │           └── HTTP POST /api/v1/sessions/{id}/messages
      │           │
      │           ├── Step 2: "summarize"
      │           │     └── PandariaRuntime::send_message(session_id, task)
      │           │           │
      │           │           └── (复用 session_id)
      │           │
      │           └── PandariaRuntime::close_session(session_id)
      │
      └── EventStore (SQLite / PostgreSQL)
            └── 每个 step 产生 WorkflowEvent（持久化、可重放）
```

### 5.3.2 Runtime trait

Tavern 通过 `Runtime` trait 抽象 Agent 执行：

```rust
#[async_trait::async_trait]
pub trait Runtime: Send + Sync {
    async fn create_session(&self, system_prompt: &str, model: &str) -> Result<String, RuntimeError>;
    async fn send_message(&self, session_id: &str, content: &str) -> Result<String, RuntimeError>;
    async fn close_session(&self, session_id: &str) -> Result<(), RuntimeError>;

    /// 便捷方法：PerStep 模式（创建 → 单条 → 关闭）
    async fn execute(&self, agent_id: &str, task: &str, ...) -> Result<Value, RuntimeError> {
        // 默认实现：三步走
    }
}
```

### 5.3.3 Session 策略

```rust
pub enum SessionStrategy {
    PerStep,                    // 每个 step 独立 session（默认）
    PerExecution,               // 同一 workflow 内共享 session（推荐）
    External(String),           // 外部传入 session_id
}
```

**PerExecution 模式优势**：
- 多个 step 在同一个 Pandaria session 中执行
- Emerald 记忆在同一 entity 下，自动建立跨 step 事实连接
- 减少 HTTP round-trip（无需重复创建/销毁 session）

### 5.3.4 PandariaRuntime 实现

```rust
pub struct PandariaRuntime {
    client: reqwest::Client,
    base_url: String,      // Pandaria api-gateway 地址
    auth_token: Option<String>,  // HMAC-SHA256 Bearer token
}

impl PandariaRuntime {
    pub fn new(base_url: impl Into<String>) -> Result<Self, PandariaError> {
        // 自动从环境变量读取认证信息
        // PANDARIA_AUTH_TOKEN 或 PANDARIA_AUTH_SECRET（自动生成 token）
    }
}
```

### 5.3.5 认证

有两种方式：

1. **直接 token**：设置 `PANDARIA_AUTH_TOKEN`
2. **自动生成**：设置 `PANDARIA_AUTH_SECRET` + `PANDARIA_TENANT_ID`，`PandariaRuntime` 自动生成 HMAC-SHA256 Bearer token

Token 格式：`base64url(payload).base64url(signature)`，其中 payload 包含 `tenant_id`、`iat`、`exp`。

### 5.3.6 配置示例

```bash
# Tavern 环境变量
RUNTIME_URL=http://pandaria:8080        # Pandaria api-gateway 地址
PANDARIA_AUTH_SECRET=my-32-byte-secret  # HMAC 密钥
PANDARIA_TENANT_ID=my-org               # 租户标识

# Agent 配置目录
AGENT_CONFIG_DIR=./configs/agents
# Workflow 配置目录
WORKFLOW_CONFIG_DIR=./configs/workflows
```

---

## 5.4 Constell — 可观测性平台

### 5.4.1 架构

```
Pandaria (agent-core)
      │
      │  #[cfg(feature = "constell")]
      │  constell_reporter.turn_end(...)
      │  constell_reporter.tool_call_end(...)
      │
      ▼
pandaria-constell-reporter (独立 crate)
      │
      ├── mpsc::UnboundedSender  →  缓冲区 1024
      │       │
      │       └── background task  →  POST /api/public/ingestion
      │                                     │
      │                                     ▼
      │                                  Constell
      │
      └── 发送失败 → warn!() + 丢弃
```

### 5.4.2 事件映射

| Pandaria 事件 | Constell 类型 | 说明 |
|---------------|--------------|------|
| Session 创建 | `trace` (name=`session:{id}`) | metadata: tenant_id, model |
| Turn 开始/结束 | `span` (parent=trace) | input=user message, output=assistant response, usage |
| Tool call | `generation` (parent=span) | input=params, output=result, metadata.success |
| Agent end | trace 的 output + metadata | total_turns, total_tokens, duration_ms |
| Error | `span` level=ERROR | statusMessage |

### 5.4.3 配置

```rust
let config = ConstellConfig {
    base_url: "http://constell:3000".into(),
    api_key: "pk-...".into(),
    buffer_size: 1024,
    batch_interval_ms: 1000,
    enabled: true,
};

let reporter = ConstellReporter::new(config);
```

### 5.4.4 与 Pandaria tracing 的关系

Pandaria 内部使用 `tokio-tracing` 做本地日志和调试。`ConstellReporter` 是独立的观测通道：

- **Pandaria tracing**：开发环境调试、本地性能分析
- **ConstellReporter**：生产环境全链路 trace、跨服务关联、成本分析

两者并行运行，不互斥。`ConstellReporter` 通过 feature gate (`#[cfg(feature = "constell")]`) 控制是否编译。

### 5.4.5 故障处理

| 场景 | 行为 |
|------|------|
| Constell 不可达 | background task 打 warn log，丢弃当前 batch |
| Channel 满（1024 条积压） | 新事件丢弃（unbounded channel 不会满，但若改用 bounded 则丢弃） |
| Reporter 未初始化（feature 关闭） | 所有调用点编译时消除（`#[cfg]`），零开销 |
| Shutdown | `ConstellReporter::shutdown()` flush 剩余 batch |

---

## 5.5 跨项目集成链路清单

| # | 链路 | 状态 | 关键文件 |
|---|------|:--:|------|
| 1 | Pandaria → Emerald (memory) | ✅ | `agent-core/src/memory/emerald.rs` |
| 2 | Pandaria → Pawbun (tools) | 📋 | `agent-core/src/tools/pawbun_adapter.rs` (新增) |
| 3 | Tavern → Pandaria (runtime) | ✅ | `tavern-adapters/src/pandaria.rs` |
| 4 | Pandaria → Constell (observability) | 📋 | `crates/pandaria-constell-reporter/` (新增) |
| 5 | Tavern → Emerald (via Pandaria) | ✅ | 通过 Pandaria 的 MemoryStore 间接集成 |
| 6 | All → Constell | 📋 | 各项目独立接入 Constell ingestion API |

---

## 5.6 下一步

- 理解全栈部署 → [第六章：部署与运维](./06-deployment.md)
- 查看集成深化 Spec → [2026-05-28-ecosystem-integration-deepening.md](../specs/2026-05-28-ecosystem-integration-deepening.md)
