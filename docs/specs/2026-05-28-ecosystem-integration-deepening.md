# Pandaria 生态集成深化 Spec

> **Version:** 1.0  
> **Date:** 2026-05-28  
> **Status:** Draft — 待评审  
> **Reference:** `docs/ecosystem.md`, AGENTS.md (ADR-003, ADR-004)

---

## 1. 目标

将 Pandaria 生态从「概念上连通、代码上孤立」（当前集成度 ≈30%）演进为**端到端可验证、每条链路可追踪、跨项目 API 有正式契约**的生产级集成。

**核心指标**：

| 指标 | 当前 | 目标 |
|------|------|------|
| 跨项目代码依赖 | 1/4 链路（仅 Emerald） | 4/4 链路全部代码级验证 |
| 可观测性覆盖 | 0（无 Constell 集成） | Pandaria + Tavern 全链路 trace |
| 跨项目 API 契约 | 0（口头约定） | 核心 API 有机器可消费 schema |
| 端到端集成测试 | 0 条 | ≥1 条覆盖完整链路 |
| 同步阻塞记忆调用 | 是（阻塞 agent loop） | 异步 fire-and-forget |

---

## 2. 当前状态评估

### 2.1 集成矩阵

| 关系 | 文档状态 | 代码状态 | 测试覆盖 | 问题 |
|------|:--:|:--:|:--:|------|
| **Pandaria → Emerald** | ✅ Spec 齐全 | ✅ `EmeraldMemoryStore` 已实现 | ✅ 7 单元测试 | 同步 HTTP 阻塞 agent loop |
| **Pandaria → Pawbun** | ⚠️ README 提及 | ❌ 无 Cargo 依赖 | ❌ 0 | Pawbun 208 测试全在隔离环境 |
| **Tavern → Pandaria** | ✅ Spec + README | ✅ `PandariaRuntime` 可用 | ✅ 适配器测试 | 每 step 创建/销毁 session，无复用 |
| **全部 → Constell** | ⚠️ ecosystem.md 提及 | ❌ 零集成代码 | ❌ 0 | 跨项目 trace 完全黑盒 |

### 2.2 架构债务

| 债务 | 影响 |
|------|------|
| 三种语言无共享类型定义 | 每个集成需手写 HTTP client + JSON 序列化 + 错误映射；API 变更靠人眼发现 |
| Pawbun 与 Pandaria 无代码关联 | 工具 trait 设计是否适配 agent loop 从未在真实上下文中验证 |
| Emerald 同步阻塞 | `on_turn_end` hook 中 HTTP 调用阻塞整个 agent loop |
| Tavern session 粒度过细 | 无法利用 Pandaria 多 turn 能力；Emerald 记忆跨 step 断裂 |
| 无可观测性桥接 | 部署后无跨项目 trace 视图 |

---

## 3. 集成链路详细规格

### 3.1 链路 A：Pandaria ← Pawbun（工具标准化）

**优先级**：P0  
**目标分支**：`pandaria: feat/v0.3.0-pawbun-integration`  
**依赖**：无外部依赖，纯代码集成

#### 3.1.1 现状

Pandaria 的 `ToolExecutor` 直接使用内联的 `AgentTool` trait 实现，工具集在编译期硬编码。Pawbun 提供了一套完整的 `Tool` / `ToolRegistry` / `ToolKit` 抽象，含 208 个测试——但 Pandaria 的 `Cargo.toml` 没有 `pawbun-toolkit` 依赖。

#### 3.1.2 需求

1. Pandaria 的 `agent-core` 添加对 `pawbun-toolkit` 的依赖
2. `AgentTool` trait 与 `Tool` trait 建立适配层（adapter / blanket impl）
3. Pawbun 内置工具（`FileReadTool`、`FileWriteTool`、`DirectoryListTool`、`WebFetchTool`、`WebSearchTool` 等）可在 Pandaria session 中直接使用
4. Pawbun 的 MCP 客户端（`DynamicTool`）可在 Pandaria 中代理远程 MCP 工具
5. 所有 Pawbun 内置工具的 path guard / SSRF 防护与 Pandaria 的 `DefaultHookDispatcher` 正确交互

#### 3.1.3 设计决策

**方案：Adapter trait，同时支持同步和异步 Tool**

Pawbun 的 `Tool` trait 有两个变体：`Tool`（同步 `execute`）和 `AsyncTool`（异步 `execute_async`）。适配层需要同时覆盖两者。Pawbun 本身不感知 Pandaria。

Pandaria 侧新增适配层：

```rust
// agent-core/src/tools/pawbun_adapter.rs（新增文件）

/// 将 pawbun_toolkit 的工具适配为 Pandaria 的 AgentTool。
/// 内部存储两种 variant：纯同步 Tool 通过 spawn_blocking 执行。
pub struct PawbunToolAdapter {
    inner: PawbunToolVariant,
    tenant_id: String,
    session_id: String,
}

enum PawbunToolVariant {
    /// pawbun_toolkit::Tool（同步 execute），通过 spawn_blocking 转为异步
    Sync(Arc<dyn pawbun_toolkit::Tool>),
    /// pawbun_toolkit::AsyncTool（异步 execute_async），原生 async
    Async(Arc<dyn pawbun_toolkit::AsyncTool>),
}

impl PawbunToolAdapter {
    pub fn new_sync(tool: Arc<dyn pawbun_toolkit::Tool>, tenant_id: String, session_id: String) -> Self {
        Self { inner: PawbunToolVariant::Sync(tool), tenant_id, session_id }
    }

    pub fn new_async(tool: Arc<dyn pawbun_toolkit::AsyncTool>, tenant_id: String, session_id: String) -> Self {
        Self { inner: PawbunToolVariant::Async(tool), tenant_id, session_id }
    }

    fn name_inner(&self) -> &str {
        match &self.inner {
            PawbunToolVariant::Sync(t) => t.name(),
            PawbunToolVariant::Async(t) => t.name(),
        }
    }

    fn description_inner(&self) -> &str {
        match &self.inner {
            PawbunToolVariant::Sync(t) => t.description(),
            PawbunToolVariant::Async(t) => t.description(),
        }
    }

    fn parameters_inner(&self) -> Vec<pawbun_toolkit::ToolParameter> {
        match &self.inner {
            PawbunToolVariant::Sync(t) => t.parameters(),
            PawbunToolVariant::Async(t) => t.parameters(),
        }
    }
}

impl AgentTool for PawbunToolAdapter {
    fn name(&self) -> &str { self.name_inner() }
    fn description(&self) -> &str { self.description_inner() }
    fn parameters(&self) -> Vec<ToolParameter> {
        self.parameters_inner().into_iter().map(Into::into).collect()
    }
    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, AgentToolError> {
        let result = match &self.inner {
            PawbunToolVariant::Sync(tool) => {
                let tool_ref = Arc::clone(tool);
                let input_clone = input.clone();
                tokio::task::spawn_blocking(move || tool_ref.execute(input_clone))
                    .await
                    .map_err(|e| AgentToolError::execution(format!("spawn_blocking failed: {e}")))??
            }
            PawbunToolVariant::Async(tool) => {
                tool.execute_async(input).await
                    .map_err(|e| AgentToolError::execution(e.to_string()))?
            }
        };
        Ok(AgentToolResult {
            content: result.content.into_iter().map(Into::into).collect(),
            is_error: false,
            details: result.metadata.unwrap_or_default(),
        })
    }
}
```

**选择 `spawn_blocking` 而非要求全部工具实现 AsyncTool 的理由**：Pawbun 的大多数内置工具（`FileRead`、`FileWrite` 等）是同步 I/O，强制要求 `AsyncTool` 会导致不必要的 trait 实现负担。`spawn_blocking` 将同步工具隔离到 tokio 阻塞线程池，不阻塞 async executor。

**为什么不直接让 Pawbun 依赖 Pandaria？**  
违反依赖方向（库不应依赖消费方）。适配层应在消费方实现。

**为什么不在 Pawbun 加 Pandaria feature？**  
Pawbun 是通用库。标注「Pandaria 生态」是定位声明，不应成为代码耦合。

#### 3.1.4 集成范围

**Phase 1（本 spec 范围）**：直接移植 Pawbun 内置工具

Pandaria 当前内置的 `FileReadTool` / `FileWriteTool` / `DirectoryListTool` 替换为 Pawbun 版本。现有 hook 交互（`PathGuard` 在 `on_tool_call` 中拦截）保持不变。

**Phase 2（后续）**：Pawbun MCP 客户端接入

`PandariaRuntime`（tavern-adapters 中）或 `SessionActor` 可直接使用 `pawbun-toolkit` 的 MCP 客户端模块连接外部 MCP 服务器，将远程工具代理为本地 `AgentTool`。

**Phase 3（后续）**：ToolConfig 动态注入

结合已有的 `HttpProxyTool` 机制，外部编排器可在创建 session 时指定使用 Pawbun 内置工具 + 自定义 HTTP 工具的组合。

#### 3.1.5 文件变更

| 文件 | 变更 |
|------|------|
| `agent-core/Cargo.toml` | 添加 `pawbun-toolkit` 依赖 |
| `agent-core/src/tools/mod.rs` | 导出 `pawbun_adapter` 模块 |
| `agent-core/src/tools/pawbun_adapter.rs` | **新增** — `PawbunToolAdapter` + 类型转换 |
| `agent-core/src/harness/session_actor.rs` | tool 构建时使用 Pawbun 内置工具（替换当前内联实现） |

#### 3.1.6 验收标准

- [ ] `cargo build -p agent-core` 编译通过（含 `pawbun-toolkit` 依赖）
- [ ] 现有 Pandaria hook 测试（path_guard、tool_guard）全部通过——Pawbun 工具替换不破坏 hook 行为
- [ ] 新增测试：`PawbunToolAdapter` 将 `ToolResult` 正确映射为 `AgentToolResult`
- [ ] 新增测试：Pawbun `ToolParameter` → Pandaria `ToolParameter` 转换正确
- [ ] `cargo clippy --workspace` 零警告

---

### 3.2 链路 B：Pandaria → Constell（可观测性桥接）

**优先级**：P0  
**目标分支**：`pandaria: feat/v0.3.0-constell-reporter`

#### 3.2.1 现状

Pandaria 内部使用 `tokio-tracing` 创建 span，携带 `tenant_id` / `session_id`。hook 系统有 `on_turn_end`、`on_agent_end`、`on_tool_call`、`on_tool_result` 等观测点。但没有任何代码将这些数据发送到 Constell。

#### 3.2.2 需求

1. 在 Pandaria 中实现一个 `ConstellReporter`，将 agent 执行事件转为 Constell ingestion 格式
2. 覆盖的事件：session 创建 / turn 执行 / tool call / tool result / agent end / error
3. 异步发送，不阻塞 agent loop
4. 发送失败不影响 agent 正常运行（best-effort 语义）

#### 3.2.3 设计决策

**方案对比**：

| 方案 | 优点 | 缺点 |
|------|------|------|
| **A. tracing Layer**：实现 `tracing-subscriber` Layer，将 Pandaria 现有 span 自动转为 Constell ingestion | 零侵入 agent-core；自动继承所有现有 span；数据一致性高 | 需要理解 Pandaria 的 span 命名约定；对非 span 事件（如 session 生命周期）不适用 |
| **B. 独立 reporter crate + 手动调用**：每个 hook 点显式调用 reporter | 精确控制上报内容；覆盖非 span 事件；不依赖 span 命名约定 | 散落在 agent-core 各处的 `#[cfg(feature)]` 调用点；与 tracing span 是两条平行路径 |
| **C. 混合**：tracing Layer 处理已有 span（turn/tool call），独立 reporter 补充 session 生命周期事件 | 结合两者优点 | 复杂度最高 |

**选择方案 B（独立 reporter crate + channel 缓冲）**，理由：
- Pandaria 的 tracing span 主要用于本地调试和日志，其粒度和字段设计未针对 Constell 数据模型优化。强行映射可能丢失关键上下文（如 tool call 的 `terminate` flag、session 的 `compaction_triggered` 等）。
- Constell 集成是可选 feature，tracing Layer 方案要求所有 subscriber 路径都感知 `#[cfg(feature = "constell")]`，侵入面更大。
- 独立 reporter 的 API 面清晰，未来可以内部改用 tracing Layer 实现而不影响调用方。

**架构**：

```
agent-core::hook::on_turn_end()
      │
      ▼
ConstellReporter (新增 crate: pandaria-constell-reporter)
      │
      ├── mpsc::channel (buffer=1024)
      │       │
      │       └── background task ──► POST /api/public/ingestion (Constell)
      │
      └── 发送失败 → warn!() + 丢弃（不阻塞、不重试主路径）
```

**为什么是独立 crate 而非 hook 实现？**
- Constell 集成是可选的（feature-gated），不应成为 agent-core 的硬依赖
- 独立的 reporter 可以有自己的 HTTP client 配置、重试策略、批量发送逻辑
- 遵循 Pandaria 的依赖单向原则：agent-core ← reporter，而非 agent-core 直接依赖 Constell SDK

#### 3.2.4 Constell 数据模型映射

Pandaria 事件到 Constell ingestion 格式的映射：

| Pandaria 事件 | Constell Observation Type | 关键字段映射 |
|---------------|--------------------------|-------------|
| Session 创建 | `trace` (name=`session:{id}`) | `metadata.tenant_id`, `metadata.model` |
| Agent Loop turn | `span` (parent=trace, name=`turn:{n}`) | `input`=user message, `output`=assistant response |
| Tool call | `generation` (parent=span, name=`tool:{name}`) | `input`=tool params, `output`=tool result, `usage`=N/A |
| Tool result | 同上 `generation` 的 completion | `output`=result content, `metadata.success` |
| Agent end | trace 的 `output` + metadata | `metadata.total_turns`, `metadata.total_tokens`, `metadata.duration_ms` |
| Error | `span` level=`ERROR` | `statusMessage`=error message |

#### 3.2.5 Ingestion 请求格式

遵循 Constell ingestion API（参考 `2026-05-20-runtime-openness.md` 的 batch 规范）：

```json
{
  "batch": [
    {
      "id": "uuid",
      "type": "trace-create",
      "body": {
        "id": "trace_uuid",
        "name": "session:{session_id}",
        "userId": "{tenant_id}",
        "metadata": { "model": "claude-sonnet-4-20250514", "provider": "anthropic" },
        "timestamp": "2026-05-28T10:00:00Z"
      }
    },
    {
      "id": "uuid",
      "type": "observation-create",
      "body": {
        "id": "obs_uuid",
        "traceId": "trace_uuid",
        "type": "GENERATION",
        "name": "tool:file_read",
        "input": "{\"path\":\"/workspace/readme.md\"}",
        "output": "{\"content\":\"...\"}",
        "startTime": "2026-05-28T10:00:01Z",
        "endTime": "2026-05-28T10:00:02Z",
        "metadata": { "tenant_id": "...", "session_id": "...", "success": true }
      }
    }
  ]
}
```

#### 3.2.6 ConstellReporter 接口

```rust
// crates/pandaria-constell-reporter/src/lib.rs（新增）

pub struct ConstellConfig {
    pub base_url: String,           // Constell 地址，如 http://localhost:3000
    pub api_key: String,            // Constell API key
    pub buffer_size: usize,         // 默认 1024
    pub batch_interval_ms: u64,     // 批量发送间隔，默认 1000
    pub enabled: bool,              // feature flag
}

pub struct ConstellReporter {
    tx: mpsc::UnboundedSender<ConstellEvent>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl ConstellReporter {
    pub fn new(config: ConstellConfig) -> Self;

    /// Agent loop 开始
    pub fn trace_start(&self, tenant_id: &str, session_id: &str, model: &str);

    /// Turn 开始/结束
    pub fn turn_start(&self, trace_id: &str, turn_index: u32, user_input: &str);
    pub fn turn_end(&self, span_id: &str, output: &str, usage: Usage);

    /// Tool call 生命周期
    pub fn tool_call_start(&self, parent_span_id: &str, tool_name: &str, input: &Value);
    pub fn tool_call_end(&self, span_id: &str, output: &Value, success: bool, elapsed_ms: u64);

    /// Agent end
    pub fn trace_end(&self, trace_id: &str, total_turns: u32, total_tokens: u64, duration_ms: u64);

    /// 优雅关闭
    pub async fn shutdown(self);
}
```

#### 3.2.7 集成点

在 `SessionActor` 或 `DefaultHookDispatcher` 中注入 `ConstellReporter`（通过 feature gate）：

```rust
// agent-core 中通过 HookDispatcher 或直接注入
#[cfg(feature = "constell")]
let constell_reporter = ConstellReporter::new(config);

// 在 on_turn_end hook 中：
#[cfg(feature = "constell")]
constell_reporter.turn_end(&span_id, &output, usage);
```

#### 3.2.8 文件变更

| 文件 | 变更 |
|------|------|
| `crates/pandaria-constell-reporter/Cargo.toml` | **新增** |
| `crates/pandaria-constell-reporter/src/lib.rs` | **新增** — `ConstellReporter` 核心实现 |
| `crates/pandaria-constell-reporter/src/event.rs` | **新增** — `ConstellEvent` 枚举 |
| `crates/pandaria-constell-reporter/src/ingestion.rs` | **新增** — Constell HTTP ingestion client |
| `agent-core/Cargo.toml` | 添加可选依赖 `pandaria-constell-reporter`（feature = "constell"） |
| `agent-core/src/harness/session_actor.rs` | `#[cfg(feature = "constell")]` 注入 reporter |

#### 3.2.9 验收标准

- [ ] `cargo build -p pandaria-constell-reporter` 独立编译通过
- [ ] `cargo build -p agent-core --features constell` 编译通过
- [ ] 单元测试：`ConstellReporter` 在 channel 满时丢弃事件不 panic
- [ ] 单元测试：background task 在 Constell 不可达时 gracefully degrade
- [ ] 集成测试：启动 Constell dev 环境 → Pandaria 执行一个 turn → Constell UI 可见 trace + span
- [ ] `ConstellReporter::shutdown()` 正确 flush 剩余事件

---

### 3.3 链路 C：跨项目 API Schema 共享

**优先级**：P1  
**目标**：建立正式的跨项目 API 契约，取代当前的口头约定

#### 3.3.1 现状

所有跨项目集成依赖手写 HTTP client + 对端 Markdown 文档中描述的 API 格式。没有一个机器可消费的 schema。Emerald spec `2026-05-27-pandaria-emerald-memorystore.md` 文档质量高，但无法做 CI 兼容性检查。

#### 3.3.2 需求

1. 为核心跨项目 API 建立 OpenAPI 3.1 schema
2. Schema 文件存储在 `pandaria/docs/specs/schemas/` 下
3. CI 中做 schema 向后兼容性检查（openapi-diff 或类似工具）
4. 目标：Pandaria 侧的 HTTP client 测试可 mock 基于 schema 生成的响应

#### 3.3.2b Schema 所有权与同步策略

每个 schema 的「源」在对端项目仓库。Pandaria 仓库中存储的是**冻结快照**，对应当前兼容性矩阵（§4）中标记为 `status: current` 的版本。

| Schema 文件 | 源仓库 | 同步方式 |
|------------|--------|---------|
| `emerald-api.openapi.yaml` | Emerald | Emerald 发版时手动拷贝 → Pandaria PR 更新 |
| `pandaria-session.openapi.yaml` | Pandaria（api-gateway 是源） | 与 api-gateway 代码同步维护 |
| `constell-ingestion.openapi.yaml` | Constell | Constell 发版时手动拷贝 → Pandaria PR 更新 |

**不采用 git submodule / CI fetch 的理由**：跨项目 schema 需要冻结版本才能做兼容性检查。自动拉取最新版会引入未经评审的 schema 变更，导致 CI 误报。手动拷贝 + PR 评审是刻意设计——每次卫星项目发版都需要 Pandaria 侧有人确认「这个新版本的 API 我们的 adapter 能处理」。

#### 3.3.3 Schema 范围

| Schema 文件 | 源仓库（权威） | 消费方 | Pandaria 侧用途 | 优先级 |
|------------|--------------|--------|---------------|:---:|
| `emerald-api.openapi.yaml` | Emerald | Pandaria (EmeraldMemoryStore) | HTTP client 测试 mock + CI 兼容性检查 | P1 |
| `pandaria-session.openapi.yaml` | Pandaria (api-gateway) | Tavern (PandariaRuntime) | Tavern 侧契约测试的参考（Pandaria 是源） | P1 |
| `constell-ingestion.openapi.yaml` | Constell | Pandaria (ConstellReporter) | Reporter 数据模型校验 + CI 兼容性检查 | P1 |
| `tavern-execution.openapi.yaml` | Tavern | 外部消费者 | 参考（不在 Pandaria CI 中校验） | P2 |

#### 3.3.4 Emerald API Schema（示例节选）

```yaml
# docs/specs/schemas/emerald-api.openapi.yaml
openapi: "3.1.0"
info:
  title: Emerald Memory API
  version: "0.2.0"
  description: |
    Frozen interface for Pandaria's EmeraldMemoryStore.
    Do not introduce breaking changes without version bump and Pandaria-side coordination.

paths:
  /v1/memories:
    post:
      operationId: addMemory
      summary: Save a memory for an entity
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [content, entity_id]
              properties:
                content:
                  type: string
                  description: Markdown-formatted conversation transcript
                  maxLength: 100000
                entity_id:
                  type: string
                  description: Maps to Pandaria tenant_id
                content_type:
                  type: string
                  default: text
                  enum: [text, conversation, document, code]
                title:
                  type: string
                metadata:
                  type: object
                  description: Full passthrough, Emerald does not parse
                  additionalProperties: true
      responses:
        "200":
          description: Memory saved successfully
          content:
            application/json:
              schema:
                type: object
                properties:
                  data:
                    type: object
                    properties:
                      memory_ids:
                        type: array
                        items:
                          type: string
                      pipeline_status:
                        type: string
                        enum: [done, processing, failed]
                      extracted_count:
                        type: integer
                  meta:
                    type: object
                    properties:
                      request_id:
                        type: string
                      took_ms:
                        type: integer

  /v1/search:
    post:
      operationId: searchMemories
      # ...（同上结构）
```

#### 3.3.5 CI 集成

```yaml
# .github/workflows/schema-check.yml（示例）
schema-check:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - name: Validate OpenAPI schemas
      run: |
        npx @apideck/openapi-validator docs/specs/schemas/*.yaml
    - name: Check backward compatibility
      run: |
        npx openapi-diff <base-branch-schema> docs/specs/schemas/*.yaml
```

#### 3.3.6 验收标准

- [ ] 三个核心 schema 文件存在且通过 OpenAPI 3.1 校验
- [ ] `EmeraldMemoryStore` 的 HTTP 请求/响应可通过 schema mock 测试
- [ ] `PandariaRuntime` 的 session API 调用可对 schema 做契约测试
- [ ] CI 中有 schema 兼容性检查（至少 validator，兼容性检查可逐步加入）

---

### 3.4 链路 D：Tavern Session 生命周期优化

**优先级**：P1  
**目标分支**：`tavern: feat/v0.4.0-session-reuse`  
**依赖**：无（Pandaria 侧无需变更，Tavern 侧纯优化）

#### 3.4.1 现状

`PandariaRuntime::execute()` 对每次 agent 执行做 `create_session → send_message（单条）→ delete_session`。这带来三个问题：

1. **无法利用多 turn**：Pandaria 的 agent loop 支持 steer/follow_up/compaction——Tavern 只用了一个首轮 turn
2. **Session 创建开销**：高频编排场景（flow 并行 step）下 HTTP round-trip 延迟累积
3. **Memory 断裂**：每个 step 产生独立 session，Emerald 记忆被写入不同的 entity context，跨 step 记忆无法关联

#### 3.4.2 需求

1. `Runtime` trait 扩展 session 生命周期方法
2. Workflow 引擎层面管理 session 复用策略
3. 同一 workflow execution 内的连续 step 复用同一个 Pandaria session

#### 3.4.3 Runtime trait 扩展

```rust
// tavern-core/src/runtime.rs（修改）

#[async_trait::async_trait]
pub trait Runtime: Send + Sync {
    /// 创建 session（不立即发送消息）
    async fn create_session(
        &self,
        system_prompt: &str,
        model: &str,
    ) -> Result<String, RuntimeError>;

    /// 在已有 session 中发送消息（支持多 turn）
    async fn send_message(
        &self,
        session_id: &str,
        content: &str,
    ) -> Result<String, RuntimeError>;

    /// 关闭 session
    async fn close_session(&self, session_id: &str) -> Result<(), RuntimeError>;

    /// 便捷方法：创建 → 单条消息 → 关闭（向后兼容，当前 execute 语义）
    async fn execute(
        &self,
        agent_id: &str,
        task: &str,
        context: Option<serde_json::Value>,
        system_prompt: &str,
        model: &str,
    ) -> Result<serde_json::Value, RuntimeError> {
        // 默认实现：三步走
        let sid = self.create_session(system_prompt, model).await?;
        let result = match self.send_message(&sid, task).await {
            Ok(r) => r,
            Err(e) => {
                let _ = self.close_session(&sid).await;
                return Err(e);
            }
        };
        let _ = self.close_session(&sid).await;
        Ok(serde_json::from_str(&result).unwrap_or(serde_json::json!({"text": result})))
    }
}
```

#### 3.4.4 Workflow 引擎 Session 复用策略

```rust
// tavern-comp 中的 SessionPool（新增概念）

/// Session 复用策略
pub enum SessionStrategy {
    /// 每个 step 独立 session（当前行为，等于 execute 默认实现）
    PerStep,
    /// 同一 workflow execution 内共享一个 session
    PerExecution,
    /// 外部传入 session_id，用于跨 execution 记忆延续。
    /// 调用方负责管理 session 生命周期（创建、关闭）。
    /// 若 session 已过期或不存在，workflow step 将收到 RuntimeError。
    External(String),
}
```

**PerExecution 模式行为**：
1. Workflow 启动时 `create_session()`
2. 每个 step 调用 `send_message()`，agent 在同一个 session 上下文中执行
3. Pandaria 内部的 agent loop 处理 tool use / compaction / memory 自动运作
4. Workflow 结束时 `close_session()`

**`External` 模式约束**：
- 调用方在启动 workflow 前先创建 session，传入 session_id
- Workflow 引擎不负责创建/销毁 session——仅使用 `send_message()`
- Tavern 侧不校验 session 是否有效（Pandaria 返回 4xx 时转为 `RuntimeError`）
- 典型场景：一个长时间运行的用户对话跨多个 workflow execution 延续记忆
- 注意：Pandaria session 可能因 compaction、TTL 或显式 DELETE 而过期。External 模式的调用方应实现 session 健康检查或重创建逻辑

#### 3.4.5 对 Memory 的影响

PerExecution 模式下，所有 step 在同一 session 中执行 → 同一 `tenant_id` → Emerald `EmeraldMemoryStore.remember()` 将多 step 记忆写入同一 entity。Emerald 的知识图谱可以自动建立跨 step 的事实连接（更新/扩展/推导），而不需要 Tavern 侧做任何额外工作。

#### 3.4.6 文件变更

| 文件 | 变更 |
|------|------|
| `tavern-core/src/runtime.rs` | `Runtime` trait 拆分为三步方法 + `execute` 默认实现 |
| `tavern-adapters/src/pandaria.rs` | 实现新的 `create_session` / `send_message` / `close_session` |
| `tavern-adapters/src/mock.rs` | 更新 Mock runtime |
| `tavern-comp/src/execution.rs` | 新增 `SessionStrategy` + PerExecution 模式实现 |
| `tavern-comp/src/workflow_config.rs` | Workflow 配置支持 `session_strategy` 字段 |

#### 3.4.7 验收标准

- [ ] 现有 `execute` 语义向后兼容——所有 173 个 Tavern 测试通过
- [ ] `PerExecution` 模式下同一 execution 的多个 step 复用同一个 session_id
- [ ] `PerExecution` 模式下 workflow 结束正确清理 session
- [ ] `PerStep` 模式（默认）行为与当前完全一致
- [ ] 新增测试：session 创建失败时 workflow 不执行后续 step
- [ ] 新增测试：session 复用场景下 tool call 结果在后续 turn 中可见

---

### 3.5 链路 E：Emerald Memory 异步化

**优先级**：P2  
**目标分支**：`pandaria: feat/v0.3.0-memory-async`

#### 3.5.1 现状

`EmeraldMemoryStore` 的 `remember()` / `recall()` 在 `on_turn_end` / `on_before_agent_start` hook 中同步调用。Hook 机制是直接函数调用（ADR-003），意味着 HTTP 延迟直接阻塞 agent loop。

```
AgentLoop::run()
  └── on_turn_end()                       ← 同步 hook
        ├── Audit::on_turn_end()          ← 同步
        ├── TokenBudget::on_turn_end()    ← 同步
        ├── MemoryHook::remember()        ← ⚠️ HTTP 调用，可能 200ms+
        └── ...其他策略
```

#### 3.5.2 需求

1. `EmeraldMemoryStore::remember()` 不阻塞 agent loop
2. 请求失败不影响 turn 正常完成（best-effort）
3. 可选：批量缓冲减少 HTTP 请求频次

#### 3.5.3 设计

**方案：内部 spawn + mpsc 缓冲（remember），超时保护（recall）**

`remember` 改为 fire-and-forget；`recall` 保持同步但增加超时保护，并在 session 创建阶段 pre-warm。

```rust
// agent-core/src/memory/emerald.rs（修改）

pub struct EmeraldMemoryStore {
    client: reqwest::Client,
    base_url: String,
    // 异步缓冲通道
    tx: mpsc::UnboundedSender<MemoryOp>,
    _worker: tokio::task::JoinHandle<()>,
    // recall 超时配置
    recall_timeout: Duration,
    // pre-warm 缓存：session 创建时后台拉取，首次 recall 命中则零延迟
    profile_cache: Arc<tokio::sync::RwLock<HashMap<String, Vec<String>>>>,
}

enum MemoryOp {
    Remember { content: String, entity_id: String, metadata: Value },
}

impl EmeraldMemoryStore {
    pub fn new(base_url: String, client: reqwest::Client) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<MemoryOp>();

        let worker_client = client.clone();
        let worker_base_url = base_url.clone();
        let _worker = tokio::spawn(async move {
            let mut batch = Vec::new();
            let mut tick = tokio::time::interval(Duration::from_millis(500));

            loop {
                tokio::select! {
                    Some(op) = rx.recv() => {
                        batch.push(op);
                        if batch.len() >= 10 {
                            Self::flush_batch(&worker_client, &worker_base_url, std::mem::take(&mut batch)).await;
                        }
                    }
                    _ = tick.tick() => {
                        if !batch.is_empty() {
                            Self::flush_batch(&worker_client, &worker_base_url, std::mem::take(&mut batch)).await;
                        }
                    }
                    else => break,
                }
            }
            // channel 关闭时 flush 剩余
            if !batch.is_empty() {
                Self::flush_batch(&worker_client, &worker_base_url, batch).await;
            }
        });

        Self {
            client,
            base_url,
            tx,
            _worker,
            recall_timeout: Duration::from_millis(500),
            profile_cache: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    /// 在 session 创建时后台 pre-warm。非必须——失败不影响 session 创建。
    pub fn prewarm(&self, tenant_id: &str) {
        let client = self.client.clone();
        let base_url = self.base_url.clone();
        let tid = tenant_id.to_string();
        let cache = self.profile_cache.clone();
        tokio::spawn(async move {
            match Self::fetch_profile(&client, &base_url, &tid).await {
                Ok(results) => {
                    cache.write().await.insert(tid, results);
                }
                Err(e) => {
                    tracing::warn!(tenant_id = %tid, error = %e, "Emerald prewarm failed");
                }
            }
        });
    }

    /// 发送单条 memory 到 Emerald。
    /// 批量 worker 和 shutdown flush 共用此方法。
    async fn flush_batch(
        client: &reqwest::Client,
        base_url: &str,
        batch: Vec<MemoryOp>,
    ) {
        for op in batch {
            match op {
                MemoryOp::Remember { content, entity_id, metadata } => {
                    let url = format!("{}/v1/memories", base_url);
                    let payload = serde_json::json!({
                        "content": content,
                        "entity_id": entity_id,
                        "content_type": "conversation",
                        "metadata": metadata,
                    });
                    match client.post(&url).json(&payload).send().await {
                        Ok(resp) if resp.status().is_success() => {}
                        Ok(resp) => {
                            tracing::warn!(
                                status = %resp.status(),
                                "Emerald remember returned non-2xx"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Emerald remember HTTP failed");
                        }
                    }
                }
            }
        }
    }
}

impl MemoryStore for EmeraldMemoryStore {
    async fn remember(&self, ctx: &MemoryContext, content: &str) -> Result<(), MemoryError> {
        let _ = self.tx.send(MemoryOp::Remember {
            content: content.to_string(),
            entity_id: ctx.tenant_id.clone(),
            metadata: serde_json::json!({
                "session_id": ctx.session_id,
                "turn_index": ctx.turn_index,
                "model": ctx.model,
            }),
        });
        Ok(())
    }

    async fn recall(&self, ctx: &MemoryContext, query: &str) -> Result<Vec<String>, MemoryError> {
        // 先查 pre-warm 缓存
        if let Some(cached) = self.profile_cache.read().await.get(&ctx.tenant_id) {
            if !cached.is_empty() {
                return Ok(cached.clone());
            }
        }

        // 缓存未命中 → 同步 HTTP 调用，加超时保护
        let result = tokio::time::timeout(
            self.recall_timeout,
            Self::fetch_profile(&self.client, &self.base_url, &ctx.tenant_id),
        )
        .await
        .unwrap_or_else(|_| {
            tracing::warn!(
                tenant_id = %ctx.tenant_id,
                timeout_ms = self.recall_timeout.as_millis(),
                "Emerald recall timed out, returning empty"
            );
            Ok(Vec::new())
        })?;

        Ok(result)
    }
}
```

#### 3.5.4 权衡

| 方案 | 优点 | 缺点 |
|------|------|------|
| 同步 HTTP（当前） | 简单，失败立即感知 | 阻塞 agent loop |
| spawn 独立 task | 不阻塞 | 失败静默（需要 logging） |
| spawn + batch | 减少 HTTP 请求数 | 进程崩溃可能丢失缓冲中事件 |

本 spec 选择 spawn + batch，理由：
- `remember` 是观测型操作——Agent 不需要等记忆保存完成才能继续
- Emerald 管线本身是异步的（pipeline_status = "processing"），保存后立即可用性已有时延
- 小批量缓冲（500ms / 10 条）在保持延迟可接受的同时减少 HTTP 开销

#### 3.5.5 文件变更

| 文件 | 变更 |
|------|------|
| `agent-core/src/memory/emerald.rs` | 重构 `EmeraldMemoryStore`——内部 worker task + channel |

#### 3.5.6 验收标准

- [ ] 现有 7 个 `EmeraldMemoryStore` 单元测试全部通过
- [ ] 新增测试：`remember()` 在 channel 满时不 panic（使用 `unbounded` channel）
- [ ] 新增测试：worker task 在 Emerald 不可达时不会 crash，只打 warn log
- [ ] 新增测试：shutdown 时 flush 剩余缓冲事件（channel close → worker 退出前发送 batch）
- [ ] 新增测试：`recall` 在 Emerald 超时时返回空结果而非 error
- [ ] 新增测试：`prewarm` 后台拉取成功时，首次 `recall` 命中缓存返回预取结果
- [ ] 验证：`on_turn_end` hook 总耗时（不含 Emerald）< 10ms（通过 tracing span 验证）

---

### 3.6 链路 F：跨项目端到端集成测试

**优先级**：P2  
**目标**：建立至少一条覆盖全生态的端到端测试链路

#### 3.6.1 需求

1. 新增 `integration-tests/` 目录（或放在 pandaria 的 `tests/` 下）
2. 用 `docker-compose` 启动完整生态
3. 覆盖至少一条完整路径：Tavern workflow → Pandaria session → Pawbun tool → Emerald memory → Constell trace
4. 测试在 CI 中运行

#### 3.6.2 Docker Compose 栈

```yaml
# integration-tests/docker-compose.yml
services:
  # ── Pandaria 及其依赖 ──
  pandaria-postgres:
    image: postgres:17
    environment:
      POSTGRES_DB: pandaria
      POSTGRES_HOST_AUTH_METHOD: trust
    ports: ["5432:5432"]

  pandaria-redis:
    image: redis:7.2
    ports: ["6379:6379"]

  pandaria:
    build: ../pandaria
    depends_on: [pandaria-postgres, pandaria-redis]
    environment:
      - DATABASE_URL=postgres://postgres@pandaria-postgres:5432/pandaria
      - REDIS_URL=redis://pandaria-redis:6379
      - PANDARIA_EMERALD_URL=http://emerald:8000
      - PANDARIA_CONSTELL_URL=http://constell:3000
    ports: ["8080:8080"]

  # ── Emerald ──
  emerald:
    build: ../Emerald
    ports: ["8000:8000"]

  # ── Constell 全栈 ──
  constell-postgres:
    image: postgres:17
    environment:
      POSTGRES_DB: constell
      POSTGRES_HOST_AUTH_METHOD: trust
    ports: ["5433:5432"]  # 避免与 pandaria-postgres 冲突

  constell-clickhouse:
    image: clickhouse/clickhouse-server:25
    ports: ["8123:8123"]

  constell-redis:
    image: redis:7.2
    ports: ["6380:6379"]  # 避免与 pandaria-redis 冲突

  constell-web:
    build: ../Constell
    depends_on: [constell-postgres, constell-clickhouse, constell-redis]
    environment:
      - DATABASE_URL=postgres://postgres@constell-postgres:5432/constell
      - CLICKHOUSE_URL=http://constell-clickhouse:8123
      - REDIS_HOST=constell-redis
      - REDIS_PORT=6379
    ports: ["3000:3000"]

  constell-worker:
    build: ../Constell/worker
    depends_on: [constell-postgres, constell-clickhouse, constell-redis]
    environment:
      - DATABASE_URL=postgres://postgres@constell-postgres:5432/constell
      - CLICKHOUSE_URL=http://constell-clickhouse:8123
      - REDIS_HOST=constell-redis
      - REDIS_PORT=6379

  # ── Tavern ──
  tavern:
    build: ../Tavern
    environment:
      - RUNTIME_URL=http://pandaria:8080
    ports: ["3001:3000"]  # host 3001，避免与 constell-web 冲突
```

#### 3.6.3 测试用例（最小集）

```
Test: E2E — Tavern Workflow → Pandaria → Emerald Memory

Given: 生态全部服务健康
When:
  1. 通过 Tavern API 注册一个 workflow（2 个 step: research → summarize）
  2. 通过 Tavern API 启动该 workflow
  3. Workflow 执行完成
Then:
  1. Tavern GET /executions/:id 返回 status=completed
  2. Pandaria 内部创建了 2 个 session（或 1 个，取决于 SessionStrategy）
  3. 轮询 Emerald GET /v1/search?entity_id=... 最多 30 秒，
     直到返回至少 1 条记忆且 pipeline_status != "processing"
     （Emerald 管线异步，需等待提取完成）
  4. Constell API GET /api/public/traces 可查询到 trace
     （至少含 session span + tool call span）
```

#### 3.6.4 验收标准

- [ ] `docker-compose up --wait` 全部服务 health check 通过
- [ ] 1 条 E2E 测试通过（含 Emerald 管线异步等待 + Constell trace 轮询）
- [ ] 测试在 CI 中可运行（至少 nightly，超时设为 5 分钟）
- [ ] 若 Emerald 或 Constell 不可达，E2E 测试标记为 `#[ignore]` 而非 panic——允许部分生态不可用时不阻塞 CI

---

## 4. 版本兼容性矩阵

### 4.1 目标

维护一个机器可读的兼容性矩阵，CI 中做跨版本 smoke test。

### 4.2 Schema

```yaml
# docs/specs/compatibility-matrix.yaml
version: "1.0"
last_updated: "2026-05-28"

matrix:
  - pandaria: "0.2.x"
    emerald: "0.2.0"
    pawbun: "0.2.x"
    tavern: "0.2.x - 0.3.x"
    constell: "0.3.x"
    status: current
    notes: "Emerald adapter (memory), Tavern adapter (runtime). No Pawbun/Constell code integration."

  - pandaria: "0.3.x"
    emerald: "0.2.0"
    pawbun: "0.2.x"
    tavern: "0.4.x"       # session reuse
    constell: "0.3.x"
    status: target
    notes: "All 4 integrations active. Constell reporter, Pawbun code dep, Tavern session reuse."

  - pandaria: "0.3.x"
    emerald: "0.3.x"      # future: async search, profile cache
    pawbun: "0.3.x"       # future: benchmarks, API audit
    tavern: "0.4.x"
    constell: "0.5.x"     # future: metrics dashboard
    status: planned
    notes: "Production-ready with metrics, evaluation, stable APIs."
```

### 4.3 CI 实现

```yaml
# .github/workflows/ecosystem-compat.yml（示例）
name: Ecosystem Compatibility
on:
  schedule:
    - cron: "0 6 * * 1"  # 每周一跑

jobs:
  compat-smoke:
    strategy:
      matrix:
        include:
          - pandaria: "main"
            emerald: "main"
            pawbun: "main"
            tavern: "main"
            constell: "main"
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4  # pandaria
      - name: Start ecosystem
        run: docker compose -f integration-tests/docker-compose.yml up -d --wait
      - name: Run E2E test
        run: cargo test -p integration-tests -- e2e_ecosystem_smoke
```

---

## 5. 实施阶段规划

```
Phase 1 ─── P0 项（打通核心，可验证）
│
├── 3.1 Pandaria ← Pawbun 代码依赖      ── 1-2 周
├── 3.2 Pandaria → Constell Reporter    ── 1-2 周
│
│  Exit: cargo build 全绿，4/4 链路有代码级验证
│  Note: 3.2 的 channel + background task 模式将直接复用到 3.5
│
Phase 2 ─── P1 项（契约 & 优化）
│
├── 3.3 跨项目 API Schema               ── 1 周
├── 3.4 Tavern Session 复用             ── 1 周
│
│  Exit: CI 中有 schema 兼容性检查，Tavern 按 execution 复用 session
│
Phase 3 ─── P2 项（生产就绪）
│
├── 3.5 Emerald Memory 异步化           ── 0.5 周（复用 Phase 1 的 channel 模式）
├── 3.6 跨项目 E2E 集成测试             ── 1 周
├── 4.x 版本兼容性矩阵 CI                ── 0.5 周
│
│  Exit: docker-compose up --wait → E2E 测试 → 绿色
```

**总估算**：6–8 周（不含排队等待）

---

## 6. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|------|:--:|------|------|
| Pawbun `Tool` trait 与 Pandaria `AgentTool` 语义差异 | 中 | 需要额外适配层或 trait 修改 | Phase 1 早期做 spike：先用一个工具做适配验证 |
| Constell ingestion API 在 v0.3.x → v0.4.x 间变动 | 中 | Reporter 需跟着改 | Constell 的 ingestion API 在 v0.3.0 已稳定，P0 API contract 冻结概率高 |
| 跨项目 docker-compose 启动慢 | 高 | CI 超时 | 使用预构建镜像 + healthcheck 依赖 |
| Emerald 侧 API v0.2.0 → v0.3.0 breaking change | 低 | Pandaria adapter 需更新 | Schema 兼容性检查 CI 会在 Phase 2 捕获 |
| Tavern Runtime trait 拆分会 break 176 个测试 | 中 | 需要较大范围改动 | `execute` 保留为默认实现（trait 提供默认方法体），现有调用方零改动 |

---

## 7. 非目标（不做的事）

- ❌ 不改变 Pawbun 的 crate 定位（保持为通用库，不引入 Pandaria 依赖）
- ❌ 不要求 Emerald 感知 Pandaria（Emerald API 保持通用）
- ❌ 不要求 Constell 做任何 Pandaria 特定适配（通过标准 SDK 接入）
- ❌ 不在本 spec 中引入新的卫星项目
- ❌ 不要求立即做 gRPC 替换 HTTP（跨语言 gRPC 是 Phase 4+ 的事）

---

## 8. 外部变更响应机制

本 spec 涉及 5 个独立演进的项目。当任一卫星项目发生变更时，spec 必须具备明确的响应路径，否则会退化为过时文档。

### 8.1 文档分层：契约 vs 插图

spec 中的内容分为两类，对变更的敏感度不同：

| 层次 | 性质 | 例子 | 外部变更时 |
|------|------|------|-----------|
| **架构决策** | 方向性约束，不绑定具体版本 | Adapter 模式、独立 reporter crate、fire-and-forget 语义 | 不受影响——架构方向不因对端 API 改版本号而变 |
| **接口契约** | 对端 API 的快照描述 | Emerald API 请求格式、Constell ingestion batch 结构 | 需要更新——快照必须与对端实际接口一致 |
| **实现示意** | 辅助理解的伪代码，不要求逐字一致 | PawbunToolAdapter 的示例代码、Channel worker 的 loop 骨架 | 可能过时但不影响正确性——实现阶段以实际代码为准 |

**规则**：spec 正文中标记「接口契约」的部分必须注明对应项目的**冻结版本号**（如 `Emerald v0.2.0`），并在兼容性矩阵中有对应条目。

### 8.2 变更传播流程

当任一卫星项目发布新版本时，按以下流程响应：

```
卫星项目发布新版本
      │
      ▼
┌─────────────────────────────────────────────┐
│ Step 1: 评估变更类型                          │
│                                             │
│  Breaking change（API 签名 / 数据模型变化）     │
│    → Step 2                                 │
│  Non-breaking change（新增字段 / 新增端点）     │
│    → 可能无需更新 spec，但需更新兼容性矩阵       │
│  Internal change（内部重构，对外接口不变）       │
│    → 无需操作                                │
└─────────────────────────────────────────────┘
      │ (breaking change)
      ▼
┌─────────────────────────────────────────────┐
│ Step 2: 更新本 spec                          │
│                                             │
│  2a. 更新 §3 中对应链路的接口契约部分           │
│  2b. 更新 §4 兼容性矩阵（新增版本行）           │
│  2c. 若适配层代码需变更，在受影响链路追加        │
│      「变更说明」子节，描述适配层需要的修改       │
│  2d. 更新 spec 头部的 Date 和 Reference        │
└─────────────────────────────────────────────┘
      │
      ▼
┌─────────────────────────────────────────────┐
│ Step 3: CI 自动检测                          │
│                                             │
│  3a. Schema 兼容性检查（Phase 2 后生效）       │
│      openapi-diff 对比 schema 文件与对端实际 API│
│      若检测到 breaking change → CI 告警       │
│                                             │
│  3b. E2E smoke test（Phase 3 后生效）         │
│      周检 docker-compose 启动全栈 + 跑核心用例   │
│      若失败 → CI 告警                         │
│                                             │
│  3c. Cargo dependency check                 │
│      dependabot/renovate 检测 Pawbun 新版本    │
│      自动提 PR 更新 Cargo.toml                │
└─────────────────────────────────────────────┘
      │
      ▼
┌─────────────────────────────────────────────┐
│ Step 4: 代码适配                             │
│                                             │
│  根据 Step 2 的变更说明修改适配层代码：          │
│  · Emerald Adapter（agent-core/memory）      │
│  · Pawbun Adapter（agent-core/tools）        │
│  · Constell Reporter（pandaria-constell-*）  │
│  · Tavern PandariaRuntime（tavern-adapters） │
│                                             │
│  所有适配层变更须有对应单元测试更新              │
└─────────────────────────────────────────────┘
```

### 8.3 各项目变更的具体影响面

| 卫星项目变更 | 影响 Pandaria 的什么 | 谁负责响应 |
|-------------|-------------------|-----------|
| **Emerald** API breaking change | `EmeraldMemoryStore` HTTP client、spec §3.3 schema、spec §3.5 异步化设计 | Pandaria 侧（Emerald 不感知 Pandaria） |
| **Emerald** 新增端点（如 `/v1/profile` 增强） | 可选——`EmeraldMemoryStore` 可新增调用，但非必须 | Pandaria 侧评估后决定 |
| **Pawbun** `Tool` trait 改名/改签名 | `PawbunToolAdapter` 实现代码、spec §3.1 | Pandaria 侧（Pawbun 不感知 Pandaria） |
| **Pawbun** 新增 crate（如 `pawbun-rag`） | 可选——Pandaria 可新增依赖，但非必须 | Pandaria 侧评估后决定 |
| **Tavern** `Runtime` trait 变更 | 不影响 Pandaria（Pandaria 不依赖 Tavern）。spec §3.4 可能需要重写 | Tavern 侧主导，Pandaria 侧确认 Session API 无 breaking change |
| **Tavern** 新增编排模式 | 不影响 Pandaria。spec 可追加新集成场景 | 各自独立 |
| **Constell** ingestion API 变更 | `ConstellReporter` 数据映射代码、spec §3.2 | Pandaria 侧（Constell 不感知 Pandaria） |
| **Constell** 新增 SDK 语言支持 | 不影响——Pandaria 通过 HTTP 而非 SDK 接入 | 无需操作 |

### 8.4 自检清单（每次 Pandaria 发版前执行）

- [ ] 兼容性矩阵（§4）中所有 `status: current` 行与实际依赖版本一致
- [ ] Schema 文件（`docs/specs/schemas/*.yaml`）与对端实际 API 一致
- [ ] 若任一卫星项目自上次 Pandaria 发版以来有 breaking change，spec 已更新（Step 2）
- [ ] CI 中 E2E smoke test 最近一次运行通过
- [ ] `Cargo.toml` 中 `pawbun-*` 依赖版本与兼容性矩阵一致

---

*本 spec 随实施进展更新各阶段的 Status 标签。Phase 启动时标记为 `In Progress`，完成时标记为 `Completed ✅`。*
