# Spec: Runtime & Memory 架构重构

**Date:** 2026-05-21
**Status:** Completed ✅ — delivered in v0.2.0
**Reference:** AGENTS.md (ADR-001 ~ ADR-005)

---

## 1. 背景与动机

### 1.1 当前问题

`TenantManagerImpl::create_session()` 目前是一个约 170 行的"上帝方法"，承担了三个不应混合的职责：

1. **租户配额守卫**：检查 tenant 存在、预留 session slot
2. **依赖注入**：把 provider、store、media_provider 等塞给 session
3. **组件组装**：创建 `DefaultHookDispatcher`、配置 path_guard/tool_guard、创建 `CompactionActor`、加载 skills、决定 tools 列表

这导致每新增一个 per-session 组件（如 memory、自定义 tool、新的 hook 策略），都必须打开 `tenant` crate 修改 `TenantManagerImpl`。

### 1.2 Memory 需求

Pandaria 目前没有 memory 系统。Compaction 只能做 session-local 的有损 summarize，无法跨 session 检索。我们需要一个可插拔的 memory 机制，支持外挂外部 memory 系统（如 SuperMemory、Mem0、自研服务等）。

**关键约束**：Pandaria 不实现完整的 memory 存储/检索/embedding，只定义协议边界和 Hook 集成点。具体的存储实现由外部系统负责。

---

## 2. 设计目标

1. **Tenant 层瘦身**：`tenant` 只负责多租户编排（配额、注册表、生命周期），不碰 session 内部组装
2. **Runtime 子模块**：在 `agent-core` 内引入 `runtime/` 子模块，统一负责 session 组件的组装
3. **Memory 协议内置**：`agent-core` 定义 `MemoryStore` trait 和 Hook 集成，实现外挂
4. **向后兼容**：现有 `SessionActor::new(SessionConfig)` 签名不变；`TenantManagerImpl` 通过 `RuntimeConfig` 聚合所有依赖配置，原有 builder 方法一次性移除
5. **无新增 crate**：通过子模块重组解决职责混乱，不引入 `runtime` 或 `memory` 独立 crate，保持编译图简洁

---

## 3. 架构变更概览

### 3.1 变更前

```
api-gateway → tenant ──→ agent-core → ai-provider
              │ create_session()    SessionActor::new(SessionConfig)
              │ 创建+组装全部在这里  只接收组装好的产物
              ↓
         storage
```

### 3.2 变更后

```
api-gateway → tenant ──→ agent-core ──→ ai-provider
              │          ├─ runtime/    SessionBuilder
              │          ├─ memory/     MemoryStore trait + MemoryHookDispatcher
              │          └─ harness/    SessionActor (不变)
              ↓
         storage

外部 Memory 系统（SuperMemory / Mem0 / 自研）
    ↓ 实现 MemoryStore trait
agent-core::memory::MemoryStore
```

---

## 4. 模块边界重新定义

| 模块 | 职责 | 不做什么 |
|------|------|---------|
| **tenant** | 租户注册、配额守卫（`TenantRegistry` / `TenantSupervisor`）、session 句柄管理（`ActiveSession` 注册表）、API 透传 | 不创建 `DefaultHookDispatcher`、不加载 skills、不决定 tools 列表、不组装 `SessionConfig` |
| **agent-core::runtime** | 接收基础设施依赖（`RuntimeConfig`），提供 `SessionBuilder` 一键组装 session | 不运行 agent loop、不定义核心协议类型 |
| **agent-core::memory** | 定义 `MemoryStore` trait、核心类型（`MemoryFact` / `MemoryQuery`）、`MemoryHookDispatcher` | 不提供任何存储实现、不做 embedding、不做向量索引 |
| **agent-core::harness** | `SessionActor`、`AgentLoop`、`ToolExecutor`、`CompactionActor` —— 拿到配置后如何跑 | 不决定配置从哪来 |
| **agent-core::hook** | `HookDispatcher` trait、上下文类型、mutation 类型、`DefaultHookDispatcher`（默认策略） | 不决定哪些 dispatcher 被组合 |
| **storage** | `SessionStore` 实现（Pg/Redis） | 不实现 `MemoryStore` |

---

## 5. Memory 系统设计

### 5.1 设计哲学：协议内置，实现外挂

Pandaria 的 `memory` 模块只提供：
- **协议**：`MemoryStore` trait + `MemoryFact` / `MemoryQuery` 类型
- **集成**：`MemoryHookDispatcher`（实现 `HookDispatcher`），负责在 turn_end 提取、在 context 注入

外部 memory 系统只需实现 `MemoryStore` trait，即可通过 `SessionBuilder` 接入 Pandaria。

### 5.2 `MemoryStore` Trait

```rust
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// 写入记忆。失败应静默丢弃，不阻塞 agent loop。
    async fn remember(&self, ctx: &MemoryContext, facts: &[MemoryFact]) -> Result<(), MemoryError>;

    /// 检索记忆。返回的事实将直接注入 LLM 上下文。
    async fn recall(&self, ctx: &MemoryContext, query: &MemoryQuery) -> Result<Vec<MemoryFact>, MemoryError>;

    /// 可选：删除某 session 相关的记忆（session 删除时调用）。
    /// 默认空实现，外部系统可不支持。
    async fn forget_session(&self, ctx: &MemoryContext) -> Result<(), MemoryError> {
        Ok(())
    }
}
```

### 5.3 核心类型

```rust
/// Pandaria 传递给外部系统的上下文标识
pub struct MemoryContext {
    pub tenant_id: String,
    pub session_id: String,
    /// Pandaria 当前没有独立的 user 层级，此字段由外部 adapter
    /// 根据 tenant_id 自行映射，或 future 从 Tenant 配置/API 请求头透传。
    pub user_id: Option<String>,
}

/// 单条记忆事实。外部系统自行决定如何索引/向量化 content。
pub struct MemoryFact {
    pub id: Option<String>,
    pub content: String,
    pub category: Option<String>,
    pub importance: Option<u8>,
    pub metadata: serde_json::Value,
}

/// 检索请求
pub struct MemoryQuery {
    pub text: String,
    pub limit: usize,
    pub session_only: bool,
}
```

**关键设计决策**：
- **Embedding 由外部系统负责**：Pandaria 只传纯文本 `content`
- **租户隔离由外部系统负责**：Pandaria 传 `tenant_id` + `session_id`，外部系统映射到自己的隔离模型（space/collection/user）
- **写入失败静默丢弃**：`MemoryHookDispatcher::on_turn_end` 中使用 `let _ = store.remember(...).await;`，并用 `tokio::time::timeout` 包装，永不阻塞主流程
- **检索失败降级**：如果 `recall` 失败，`MemoryHookDispatcher` 返回 `ContextMutation::default()`，对话正常继续

### 5.4 `MemoryHookDispatcher`

```rust
pub struct MemoryHookDispatcher {
    store: Arc<dyn MemoryStore>,
}

#[async_trait]
impl HookDispatcher for MemoryHookDispatcher {
    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        // 1. 从本轮消息中提取关键事实
        let facts = extract_facts(&ctx.messages);
        if facts.is_empty() { return; }

        // 2. Fire-and-forget 写入外部 memory 系统
        let mem_ctx = MemoryContext {
            tenant_id: ctx.tenant_id.clone(),
            session_id: ctx.session_id.clone(),
            user_id: None,
        };
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            self.store.remember(&mem_ctx, &facts),
        ).await;
    }

    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        // 1. 用最近用户消息构建查询
        let query = build_query(&ctx.messages);
        let mem_ctx = MemoryContext {
            tenant_id: ctx.tenant_id.clone(),
            session_id: ctx.session_id.clone(),
            user_id: None,
        };

        // 2. 检索外部记忆
        let facts = match tokio::time::timeout(
            Duration::from_secs(3),
            self.store.recall(&mem_ctx, &query),
        ).await {
            Ok(Ok(facts)) if !facts.is_empty() => facts,
            _ => return ContextMutation::default(),
        };

        // 3. 注入 synthetic user message
        let memory_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![Content::Text {
                text: format!("[Memory]\n{}", format_facts(&facts)),
                text_signature: None,
            }],
            timestamp: SystemTime::now(),
        });

        let mut messages = ctx.messages.clone();
        messages.insert(0, memory_msg);
        ContextMutation { messages: Some(messages) }
    }

    async fn on_compact_end(&self, ctx: &CompactEndCtx) {
        // Compaction summary 是重要的长期记忆，写入外部 memory 系统
        if let Some(ref result) = ctx.result {
            let fact = MemoryFact {
                id: None,
                content: format!("[Session Compaction Summary]\n{}", result.summary),
                category: Some("compaction".to_string()),
                importance: Some(8),
                metadata: serde_json::json!({
                    "session_id": ctx.session_id,
                    "tokens_before": result.tokens_before,
                }),
            };
            let mem_ctx = MemoryContext {
                tenant_id: ctx.tenant_id.clone(),
                session_id: ctx.session_id.clone(),
                user_id: None,
            };
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                self.store.remember(&mem_ctx, &[fact]),
            ).await;
        }
    }
}

/// 从一轮对话消息中提取值得长期记忆的关键事实。
/// 策略：提取 assistant 最终回复（不含 tool calls）+ 重要 tool results，
/// 跳过用户原始输入（已在 conversation 中），跳过错误/异常消息。
fn extract_facts(messages: &[AgentMessage]) -> Vec<MemoryFact> { ... }

/// 用最近 1-2 轮用户消息拼接成检索查询文本。
fn build_query(messages: &[AgentMessage]) -> MemoryQuery { ... }
```

### 5.5 外部系统接入示例

外部实现可以是独立 crate、私有模块、甚至跨语言服务：

```rust
// crates/supermemory-adapter/ 或外部项目
pub struct SuperMemoryAdapter {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    // tenant_id -> space_id 内部映射
}

#[async_trait]
impl agent_core::memory::MemoryStore for SuperMemoryAdapter {
    async fn remember(&self, ctx: &MemoryContext, facts: &[MemoryFact]) -> Result<(), MemoryError> {
        let space_id = self.resolve_space(&ctx.tenant_id).await?;
        for fact in facts {
            self.client.post(format!("{}/api/add", self.base_url))
                .json(&json!({
                    "spaceId": space_id,
                    "content": fact.content,
                    "metadata": fact.metadata,
                }))
                .send().await?;
        }
        Ok(())
    }

    async fn recall(&self, ctx: &MemoryContext, query: &MemoryQuery) -> Result<Vec<MemoryFact>, MemoryError> {
        let space_id = self.resolve_space(&ctx.tenant_id).await?;
        let resp = self.client.post(format!("{}/api/search", self.base_url))
            .json(&json!({ "spaceId": space_id, "query": query.text, "limit": query.limit }))
            .send().await?;
        // 映射 resp -> Vec<MemoryFact>
    }
}
```

---

## 6. Runtime 子模块设计

### 6.1 `RuntimeConfig`

把 `TenantManagerImpl` 目前分散的依赖字段收拢为一个配置结构。

**注意**：`RuntimeConfig` 放在 `agent-core` 中，不能引用 `tenant::CostTracker`（否则引入 `agent-core → tenant` 循环依赖）。成本追踪通过 `DefaultHookConfig.cost_callback` 解耦。

```rust
/// DefaultHookDispatcher 的可配置策略字段
pub struct DefaultHookConfig {
    pub denied_tools: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub path_guard_fields: HashMap<String, Vec<String>>,
    pub path_guard_scan_unknown: bool,
    pub max_turns_per_session: usize,
    /// 媒体成本回调，由上层（如 tenant::CostTracker）包装后注入
    pub cost_callback: Option<Arc<dyn Fn(&str, f64) + Send + Sync>>,
}

impl Default for DefaultHookConfig {
    fn default() -> Self {
        Self {
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            path_guard_fields: HashMap::new(),
            path_guard_scan_unknown: false,
            max_turns_per_session: 0,
            cost_callback: None,
        }
    }
}

pub struct RuntimeConfig {
    pub provider: Arc<dyn LlmProvider>,
    pub default_model: String,
    pub default_system_prompt: String,
    /// 保留字段，当前为 dead_code，待后续 LLM 选型模块使用
    pub default_context_window: usize,

    // 可选基础设施
    pub store: Option<Arc<dyn SessionStore>>,
    pub memory_store: Option<Arc<dyn MemoryStore>>,
    pub media_provider: Option<Arc<dyn MediaProvider>>,
    pub media_registry: Option<Arc<MediaModelRegistry>>,

    // 共享客户端
    pub http_client: reqwest::Client,

    // 运行时默认配置
    pub compaction_config: CompactionConfig,
    pub agent_space: AgentSpace,
    pub hook_config: DefaultHookConfig,
}
```

### 6.2 `SessionBuilder`

```rust
pub struct SessionBuilder {
    config: RuntimeConfig,
    tenant_id: String,
    session_id: String,
    system_prompt: String,
    model: String,
    external_tools: Vec<ToolConfig>,
}

impl SessionBuilder {
    pub fn new(config: &RuntimeConfig) -> Self;
    pub fn tenant_id(mut self, id: impl Into<String>) -> Self;
    pub fn session_id(mut self, id: impl Into<String>) -> Self;
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self;
    pub fn model(mut self, model: impl Into<String>) -> Self;
    pub fn with_external_tools(mut self, tools: Vec<ToolConfig>) -> Self;

    pub async fn build(self) -> Result<BuiltSession, AgentError> {
        // 1. 创建 DefaultHookDispatcher，应用 hook_config
        let mut base = DefaultHookDispatcher::with_space(self.config.agent_space.clone());
        base.denied_tools = self.config.hook_config.denied_tools.clone();
        base.allowed_tools = self.config.hook_config.allowed_tools.clone();
        base.path_guard_fields = self.config.hook_config.path_guard_fields.clone();
        base.path_guard_scan_unknown = self.config.hook_config.path_guard_scan_unknown;
        base.max_turns_per_session = self.config.hook_config.max_turns_per_session;
        base.cost_callback = self.config.hook_config.cost_callback.clone();

        // 2. 如果有 memory_store，组合 MemoryHookDispatcher
        let dispatcher: Arc<dyn HookDispatcher> =
            if let Some(ref mem) = self.config.memory_store {
                Arc::new(CombinedDispatcher::new(vec![
                    Arc::new(base),
                    Arc::new(MemoryHookDispatcher::new(mem.clone())),
                ]))
            } else {
                Arc::new(base)
            };

        // 3. 创建 tools 列表（media + http proxy + 外部 tools）
        let mut tools: Vec<AgentToolRef> = vec![];
        // ... media tool ...
        // ... http proxy tools from external_tools ...

        // 4. 创建 CompactionActor
        let compaction_actor = Arc::new(CompactionActor::new(
            self.config.compaction_config.clone(),
            self.config.provider.clone(),
            self.model.clone(),
            Arc::new(DefaultFileOperationExtractor::default()),
        ));

        // 5. 加载 skills
        let skills = self.load_skills().await?;

        // 6. 组装 SessionActor
        let actor = SessionActor::new(SessionConfig {
            tenant_id: self.tenant_id,
            session_id: self.session_id,
            system_prompt: self.system_prompt,
            model: self.model,
            provider: self.config.provider.clone(),
            hook_dispatcher: dispatcher,
            compaction_actor,
            tools: tools.clone(),
            store: self.config.store.clone(),
            skills,
        });

        Ok(BuiltSession { actor, tools })
    }
}

/// SessionBuilder 的构建结果，包含 actor 和 tools（TenantManagerImpl 需要 tools 填充 ActiveSession）
pub struct BuiltSession {
    pub actor: SessionActor,
    pub tools: Vec<AgentToolRef>,
}
```

### 6.3 `CombinedDispatcher`

当需要组合多个 `HookDispatcher` 时使用。放置在 `agent-core/src/hook/combined.rs`：

```rust
pub struct CombinedDispatcher {
    chain: Vec<Arc<dyn HookDispatcher>>,
}

impl CombinedDispatcher {
    pub fn new(chain: Vec<Arc<dyn HookDispatcher>>) -> Self;
}

#[async_trait]
impl HookDispatcher for CombinedDispatcher {
    // 阻塞型 hook：first-block-wins，按顺序执行
    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation);
    async fn on_before_compact(&self, ctx: &CompactCtx) -> CompactDecision;

    // 链式 hook：管道模式执行。
    // 每个子 dispatcher 看到的 messages 是上一个处理后的结果，
    // 最终返回最后一个非 None mutation。这保证多个 dispatcher 可以协作修改上下文。
    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        let mut messages = ctx.messages.clone();
        for d in &self.chain {
            let mutation = d.on_context(&ContextCtx {
                tenant_id: ctx.tenant_id.clone(),
                session_id: ctx.session_id.clone(),
                messages: messages.clone(),
            }).await;
            if let Some(msgs) = mutation.messages {
                messages = msgs;
            }
        }
        ContextMutation { messages: Some(messages) }
    }

    // 其他链式 hook 采用"逐字段 Option 覆盖"策略：
    // 顺序执行每个子 dispatcher，对于 mutation 中的每个 Option<T> 字段，
    // 后面的非 None 值覆盖前面的。messages 字段走完整管道（如上所示）。
    // on_before_agent_start、on_before_provider_request、on_after_provider_response 同理。

    // 观测型 hook：顺序执行，fire-and-forget
    async fn on_turn_end(&self, ctx: &TurnEndCtx);
    // ... 其他观测型 hook 同理
}
```

**设计决策**：`CombinedDispatcher` 放在 `agent-core`（而非 `runtime`），因为它只操作 `HookDispatcher` trait，是一个通用工具，可被任何上层复用。

---

## 7. Tenant 层瘦身

### 7.1 `TenantManagerImpl` 变更前

```rust
pub struct TenantManagerImpl {
    registry: Arc<TenantRegistry>,
    provider: Arc<dyn LlmProvider>,
    store: Option<Arc<dyn SessionStore>>,
    default_model: String,
    default_system_prompt: String,
    default_context_window: usize,
    sessions: DashMap<(String, Uuid), ActiveSession>,
    media_provider: Option<Arc<dyn MediaProvider>>,
    media_registry: Option<Arc<MediaModelRegistry>>,
    cost_tracker: Option<Arc<CostTracker>>,
    http_client: Option<reqwest::Client>,
    available_models: Vec<String>,
    max_sync_wait_ms: u64,
}
```

`create_session()` 约 170 行，包含 dispatcher 创建、tool 创建、compaction 配置、skills 加载等全部组装逻辑。

### 7.2 `TenantManagerImpl` 变更后

```rust
pub struct TenantManagerImpl {
    registry: Arc<TenantRegistry>,
    runtime_config: Arc<RuntimeConfig>,
    sessions: DashMap<(String, Uuid), ActiveSession>,
    available_models: Vec<String>,
    max_sync_wait_ms: u64,
}
```

`create_session()` 缩减至约 30 行：

```rust
async fn create_session(&self, tenant_id: &str, params: CreateSessionParams) -> Result<SessionInfo, TenantError> {
    let supervisor = self.registry.get(tenant_id).ok_or(...)?;
    let guard = supervisor.reserve_session()?;
    let session_id = Uuid::new_v4();

    let built = SessionBuilder::new(&self.runtime_config)
        .tenant_id(tenant_id)
        .session_id(&session_id.to_string())
        .system_prompt(params.system_prompt.unwrap_or_else(|| self.runtime_config.default_system_prompt.clone()))
        .model(self.runtime_config.default_model.clone())
        .with_external_tools(params.tools)
        .build()
        .await
        .map_err(|e| TenantError::Internal { ... })?;

    let actor = built.actor;
    let tools = built.tools;

    // 注册 webhook、event bridge...
    // 存入 sessions map 时使用 tools 填充 ActiveSession
}

async fn delete_session(&self, tenant_id: &str, session_id: &Uuid) -> Result<(), TenantError> {
    // ... 取出 ActiveSession ...
    {
        let mut actor = entry.actor.lock().await;
        actor.shutdown().await;
    }

    // Session 删除时通知外部 memory 系统清理（可选，失败静默）
    if let Some(ref mem) = self.runtime_config.memory_store {
        let _ = mem.forget_session(&agent_core::memory::MemoryContext {
            tenant_id: tenant_id.to_string(),
            session_id: session_id.to_string(),
            user_id: None,
        }).await;
    }

    // ... 释放资源 ...
}
```

### 7.3 `TenantManagerImpl::new()` 签名变更

变更前：
```rust
pub fn new(
    registry: Arc<TenantRegistry>,
    provider: Arc<dyn LlmProvider>,
    store: Option<Arc<dyn SessionStore>>,
    default_model: impl Into<String>,
    default_system_prompt: impl Into<String>,
    default_context_window: usize,
) -> Self
```

变更后：
```rust
pub fn new(
    registry: Arc<TenantRegistry>,
    runtime_config: Arc<RuntimeConfig>,
) -> Self
```

原有 builder 方法（`with_media`、`with_cost_tracker`、`with_http_client` 等）全部移除，这些配置迁移到 `RuntimeConfig` 的构造阶段（在 `api-gateway` 启动时完成）。

---

## 8. 接口与兼容性

### 8.1 向后兼容

- `SessionActor::new(config: SessionConfig)` **签名不变**。现有测试和直接构造 `SessionActor` 的代码不受影响。
- `HookDispatcher` trait **不变**。`DefaultHookDispatcher` **不变**。
- `SessionStore` trait **不变**。
- `AgentEvent`、`AgentMessage` 等核心类型 **不变**。
- `TenantManager` trait **不变**（公开 API 不变）。

### 8.2 破坏性变更

- `TenantManagerImpl::new()` 签名变更：从接收多个分散参数变为接收 `Arc<RuntimeConfig>`
- `TenantManagerImpl` 的 builder 方法（`with_media`、`with_cost_tracker`、`with_http_client`、`with_available_models`、`with_max_sync_wait_ms`）全部移除
- `api-gateway` 的启动代码需要重构：先构造 `RuntimeConfig`，再传给 `TenantManagerImpl::new()`

### 8.3 迁移路径

1. 在 `agent-core` 中新增 `runtime/` 和 `memory/` 子模块
2. 将 `TenantManagerImpl` 中的组装逻辑逐步迁移到 `SessionBuilder`
3. 在 `api-gateway` 中重构启动代码，构造 `RuntimeConfig`
4. 删除 `TenantManagerImpl` 的废弃 builder 方法

---

## 9. 文件变更清单

### 新增文件

| 文件 | 说明 |
|------|------|
| `crates/agent-core/src/runtime/mod.rs` | Runtime 模块入口 |
| `crates/agent-core/src/runtime/config.rs` | `RuntimeConfig` |
| `crates/agent-core/src/runtime/builder.rs` | `SessionBuilder` |
| `crates/agent-core/src/memory/mod.rs` | Memory 模块入口 |
| `crates/agent-core/src/memory/store.rs` | `MemoryStore` trait |
| `crates/agent-core/src/memory/types.rs` | `MemoryContext`、`MemoryFact`、`MemoryQuery` |
| `crates/agent-core/src/memory/hook.rs` | `MemoryHookDispatcher` |
| `crates/agent-core/src/memory/extractor.rs` | 从 `AgentMessage` 提取 `MemoryFact` |
| `crates/agent-core/src/hook/combined.rs` | `CombinedDispatcher` |

### 修改文件

| 文件 | 变更 |
|------|------|
| `crates/agent-core/src/lib.rs` | 导出 `runtime`、`memory`、`hook::combined` |
| `crates/agent-core/src/hook/mod.rs` | 导出 `CombinedDispatcher` |
| `crates/agent-core/src/hook/context.rs` | `CompactEndCtx` 新增 `result: Option<crate::compaction::CompactionResult>` 字段 |
| `crates/agent-core/src/harness/session.rs` | `SessionActor` 新增 `pub fn tools(&self) -> &[AgentToolRef]`（运行时状态查询）；`run_auto_compaction()` 增加 `on_compact_end` hook 触发点 |
| `crates/tenant/src/manager.rs` | `TenantManagerImpl` 瘦身，使用 `SessionBuilder`；`delete_session()` 增加 `forget_session` 调用 |
| `crates/api-gateway/src/main.rs`（或启动文件） | 重构为构造 `RuntimeConfig` 后传给 `TenantManagerImpl` |

### 删除/废弃

| 内容 | 说明 |
|------|------|
| `TenantManagerImpl::with_media()` | 迁移到 `RuntimeConfig` |
| `TenantManagerImpl::with_cost_tracker()` | 移除；成本追踪通过 `DefaultHookConfig.cost_callback` 注入 |
| `TenantManagerImpl::with_http_client()` | 迁移到 `RuntimeConfig` |
| `TenantManagerImpl::with_available_models()` | 迁移到 `RuntimeConfig` |
| `TenantManagerImpl::with_max_sync_wait_ms()` | 迁移到 `RuntimeConfig` |

---

## 10. 与 AGENTS.md 的对应关系

| ADR/约束 | 本 spec 如何满足 |
|----------|-----------------|
| ADR-003 直接函数调用 | `MemoryHookDispatcher` 是 `HookDispatcher` 的直接实现，内部调用 `MemoryStore` 为直接函数调用，无 Actor、无 EventBus |
| ADR-004 tokio task 隔离 | session 仍然是独立 task；memory write 是 fire-and-forget，不共享 mutable state |
| ADR-005 不可裁剪能力 | Memory 协议内置，属于多租户基础能力的自然延伸；外部系统实现可替换 |
| 依赖方向严格单向 | `api-gateway → tenant → agent-core → ai-provider`，`storage` 被 `tenant` 依赖（通过 `RuntimeConfig` 透传），无反向依赖 |
| 安全约束 | memory 操作日志通过 tracing span 携带 `tenant_id`；API key 等敏感信息在外部 adapter 中管理，不进入 Pandaria 日志 |

---

## 11. 实施计划

### Phase 1：Runtime 子模块 + Tenant 瘦身（不碰 memory）

1. 新增 `agent-core/src/runtime/{mod,config,builder}.rs`
2. 新增 `agent-core/src/hook/combined.rs`
3. 重构 `TenantManagerImpl` 使用 `SessionBuilder`
4. 重构 `api-gateway` 启动代码构造 `RuntimeConfig`
5. 验证所有现有测试通过

### Phase 2：Memory 协议 + Hook 集成

1. 新增 `agent-core/src/memory/{mod,store,types,hook,extractor}.rs`
2. 在 `RuntimeConfig` 中增加 `memory_store: Option<Arc<dyn MemoryStore>>`
3. 在 `SessionBuilder` 中集成 `MemoryHookDispatcher` 组合逻辑
4. 编写 `InMemoryStore`（纯内存实现，用于测试）
5. 编写 `MemoryHookDispatcher` 的单元测试

### Phase 3：外部 Adapter 示例（可选，文档/示例性质）

1. 在 `docs/` 或 `examples/` 中提供 SuperMemory 接入示例
2. 验证外部 adapter 只需实现 `MemoryStore` trait 即可工作

---

## 12. 附录：术语表

| 术语 | 定义 |
|------|------|
| **Runtime** | `agent-core` 内负责 session 组件组装的子模块，提供 `SessionBuilder` |
| **MemoryStore** | 外部 memory 系统需实现的 trait，定义 `remember` / `recall` / `forget_session` 三个操作 |
| **MemoryHookDispatcher** | `HookDispatcher` 的实现，负责在 agent loop 中自动写入/检索 memory，同时监听 `on_compact_end` 保存 compaction summary |
| **CombinedDispatcher** | 组合多个 `HookDispatcher` 的通用实现；阻塞型 first-block-wins；链式 hook 采用管道模式（前一个的输出作为后一个的输入） |
| **RuntimeConfig** | 运行时全局配置，聚合 provider、store、memory_store、media、hook_config 等基础设施依赖 |
| **BuiltSession** | `SessionBuilder::build()` 的返回类型，包含 `SessionActor` 和 `tools` 列表 |
| **fire-and-forget** | memory 写入操作不 await 结果，失败静默丢弃，不阻塞 agent loop |
