# Spec: Persistence & E2E Test Hardening

**Date:** 2026-05-26
**Status:** Completed ✅ — delivered in v0.2.0
**Reference:** AGENTS.md (ADR-004, ADR-005)

---

## 1. 背景与动机

### 1.1 当前持久化状态

Pandaria 的 `SessionStore` trait 和 PostgreSQL / Redis 适配器已完成基本实现，但存在以下缺口：

| 问题 | 影响 |
|---|---|
| **fire-and-forget 落盘**：`prompt()` 后 `tokio::spawn` 异步保存，进程崩溃时最后一次 turn 可能丢失 | 数据可靠性不足 |
| **手动 `restore()`**：SessionActor 构造后需调用方显式调 `restore()`，容易遗漏 | 恢复逻辑分散 |
| **api-gateway 未接入**：`HarnessConfig` 中 `store: None`，HTTP API 无持久化 | session 重启即丢失 |
| **全量保存每轮**：每次 `prompt()` 后序列化全部 `SessionEntry`，大 session I/O 重 | 性能浪费 |
| **MemoryStore 独立**：删除 session 时 MemoryStore 数据不联动清理 | 孤儿数据 |

### 1.2 当前 E2E 测试状态

api-gateway 有 9 个 E2E 测试文件（~2,265 行），覆盖 session 生命周期、SSE、租户隔离、配额。但全部使用 `store: None`，缺失：

- 持久化恢复全链路测试
- compaction + persistence 组合场景
- 持久化故障注入（DB 不可用降级）
- 并发 session 压力测试
- MemoryStore 联动测试

### 1.3 Memory 与 Emerald 集成

Emerald（`../Emerald`）是外挂记忆系统。其 `add()` API 接受：

```json
{
  "content": "<formatte的 Markdown 文本>",
  "entity_id": "tenant:session",
  "content_type": "conversation",
  "metadata": { "turn_index": 3, "model": "claude-4", ... }
}
```

Emerald 内部自行完成提取、分块、嵌入、索引、关系推断。Pandaria 不应自己做 fact extraction（当前 `extract_facts()` 与 Emerald 管线重复），而应：
1. 将对话 turn 格式化为结构化 Markdown
2. 附带结构化元数据
3. 交给外挂系统处理

---

## 2. 设计目标

1. **持久化可靠性**：crash-safe（WAL 语义等价），最坏情况丢失最后 1 个未完成的 turn
2. **自动恢复**：SessionActor 构造时自动检测并恢复已有 session，无需调用方手动 `restore()`
3. **api-gateway 全链路持久化**：HTTP API 创建的 session 自动持久化，重启后可恢复
4. **增量保存**：仅序列化新追加的 entries，减少 I/O
5. **Memory 数据准备**：重新设计 Memory 模块为「对话格式化 + 元数据构建」，而非「事实提取」
6. **E2E 测试覆盖**：新增 5 个关键场景的 E2E 测试

---

## 3. 持久化加固

### 3.1 HarnessConfig 注入 SessionStore（P1）

**变更**：`api-gateway/tests/e2e/common.rs` 中 `build_test_app()` 系列函数，将 `store: None` 改为可选注入。

```rust
// 新增工厂函数
pub fn build_test_app_with_store(
    provider: Arc<dyn LlmProvider>,
    store: Arc<dyn SessionStore>,
) -> Router {
    let mut config = build_harness_config(provider);
    config.store = Some(store);
    // ...
}

pub fn build_test_app(provider: Arc<dyn LlmProvider>) -> Router {
    build_test_app_with_store(provider, /* in-memory store */)
}
```

**影响文件**：
- `crates/api-gateway/tests/e2e/common.rs`
- `crates/api-gateway/src/server.rs`（生产路径）

**验收标准**：
- E2E 测试中有 PG/Redis store 注入路径
- HTTP API 创建的 session 落盘到 PostgreSQL

### 3.2 自动 restore（P2）

**变更**：`SessionActor::new()` 内部自动调用 `restore()`。当前手动调用模式改为内部自动检测。

```rust
impl SessionActor {
    pub fn new(config: SessionConfig) -> Self {
        let mut actor = SessionActor { /* fields */ };
        // 如果配置了 store，将 restore 标记为 pending
        // 在首次 prompt() 或 run_with_messages() 时自动执行
        actor.needs_restore = config.store.is_some();
        actor
    }
}
```

**设计决策**：不在 `new()` 中直接 await（构造函数非 async），改为在首次 `prompt()` / `run_with_messages()` 的入口处懒执行 restore。

**错误处理**：恢复失败时记录 warn 日志，降级为空 session 继续执行（不阻塞 agent loop），与持久化保存失败的 fire-and-forget 语义一致。如果 store 中存在 session 数据但反序列化失败（数据损坏），清空 entries 并以空 session 启动，避免损坏数据污染后续 turn。

**迁移路径**：保留 `restore()` 公开方法，但标记为 `#[deprecated]`，内部转为空操作。未来版本移除。现有调用方（如 `crates/storage/tests/integration_postgres.rs` 中的 `session2.restore()`）改为依赖自动恢复。

**影响文件**：
- `crates/agent-core/src/harness/session.rs`
- `crates/storage/tests/integration_postgres.rs`（移除手动 restore 调用）

**验收标准**：
- 调用方不需要显式调用 `restore()`
- `SessionActor::new() + prompt()` 即可自动恢复历史
- 恢复失败时 session 正常启动，日志中有 warn 记录

### 3.3 增量保存策略（P3）

**变更**：`prompt()` 结束时的 `save_session()` 不再全量序列化，改为仅保存新增的 entries。

```rust
// SessionActor 新增字段
last_saved_entry_count: usize,

// prompt() 结束时
let new_entries = &self.entries[self.last_saved_entry_count..];
if !new_entries.is_empty() {
    // 发送增量（追加模式）或全量替换（取决于 SessionStore 实现）
    store.append_entries(&tenant_id, &session_id, new_entries).await;
    self.last_saved_entry_count = self.entries.len();
}
```

**SessionStore trait 扩展**：

```rust
#[async_trait]
pub trait SessionStore: Send + Sync {
    // 现有方法保持不变
    async fn save_session(...) -> Result<(), AgentError>;
    async fn load_session(...) -> Result<Vec<SessionEntry>, AgentError>;
    async fn delete_session(...) -> Result<(), AgentError>;
    async fn list_sessions(...) -> Result<Vec<String>, AgentError>;

    // 新增：追加 entries（默认实现 fallback 到全量 save）
    async fn append_entries(
        &self,
        tenant_id: &str,
        session_id: &str,
        new_entries: &[SessionEntry],
    ) -> Result<(), AgentError> {
        // 默认：加载 → 合并 → 保存
        let mut all = self.load_session(tenant_id, session_id).await?;
        all.extend_from_slice(new_entries);
        self.save_session(tenant_id, session_id, &all).await
    }
}
```

**命名说明**：`append_entries` 方法名反映的是调用方的意图（「我有一批新条目要追加」），而非强制所有适配器在存储层做物理追加。默认实现走 load→merge→save（全量覆盖但调用方只传增量），PostgreSQL 适配器 override 为 `jsonb_insert()` 做真正的存储层追加优化。Redis 适配器保持默认实现（Redis String 不支持原地追加 JSON 数组）。

**PostgreSQL 适配器优化**：override `append_entries`，使用 `jsonb_insert()` 在存储层原地追加。

**影响文件**：
- `crates/agent-core/src/persistence/store.rs`
- `crates/storage/src/session/postgres.rs`
- `crates/storage/src/session/redis.rs`
- `crates/agent-core/src/harness/session.rs`

**验收标准**：
- 每轮 prompt 后只序列化增量
- flush() 使用全量保存保证最终一致性

### 3.4 MemoryStore 联动删除（P4）

**变更**：`TenantManagerImpl::delete_session()` 或 `SessionStore::delete_session()` 触发时，同步调用 `MemoryStore::forget_session()`。

```rust
// tenant/manager.rs
pub async fn delete_session(&self, tenant_id: &str, session_id: &str) -> Result<(), AgentError> {
    // 1. 删除 SessionStore
    self.store.delete_session(tenant_id, session_id).await?;
    // 2. 清理 MemoryStore
    if let Some(ref mem) = self.memory_store {
        let ctx = MemoryContext { tenant_id, session_id, ... };
        let _ = mem.forget_session(&ctx).await; // fire-and-forget, 不阻塞删除
    }
    // 3. 释放 session slot
    self.registry.release_slot(tenant_id).await?;
}
```

**影响文件**：
- `crates/tenant/src/manager.rs`

**验收标准**：
- 删除 session 后 MemoryStore 对应数据被清理
- forget 失败不影响 session 删除

---

## 4. Memory 数据准备重构

### 4.1 设计原则

**Pandaria 不做 fact extraction。** Emerald 有自己的提取管线（extract → chunk → embed → index → relationship inference）。Pandaria 的职责是：

1. 将 agent turn 格式化为结构化 Markdown 文本
2. 附带结构化元数据（turn_index, model, token_usage, tool_calls）
3. 调用 MemoryStore trait 提交

### 4.2 MemoryStore trait 简化（M1）

**变更前**（当前）：

```rust
#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn remember(&self, ctx: &MemoryContext, facts: &[MemoryFact]) -> Result<(), MemoryError>;
    async fn recall(&self, ctx: &MemoryContext, query: &MemoryQuery) -> Result<Vec<MemoryFact>, MemoryError>;
    async fn forget_session(&self, ctx: &MemoryContext) -> Result<(), MemoryError> { Ok(()) }
}
```

**变更后**：

```rust
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// 发送格式化后的对话内容到外挂记忆系统。
    /// `content` 是 Markdown 格式的完整 turn 文本，供外挂系统自行提取。
    /// `metadata` 携带结构化上下文（turn_index, model, token_usage 等）。
    async fn remember(
        &self,
        ctx: &MemoryContext,
        content: &str,
        metadata: &serde_json::Value,
    ) -> Result<(), MemoryError>;

    /// 检索与查询相关的记忆，返回纯文本列表。
    async fn recall(
        &self,
        ctx: &MemoryContext,
        query: &str,
    ) -> Result<Vec<String>, MemoryError>;

    /// 删除 session 关联的所有记忆。
    async fn forget_session(&self, ctx: &MemoryContext) -> Result<(), MemoryError> {
        Ok(())
    }
}
```

**MemoryContext 增强**：

```rust
pub struct MemoryContext {
    pub tenant_id: String,
    pub session_id: String,
    pub user_id: Option<String>,
    /// 新增：session 元数据供外挂系统做路由决策。
    /// Emerald 的 entity_id 是单一字符串，但 `model` 和 `session_started_at`
    /// 在外挂系统内部可用于按模型/时间范围过滤记忆，无需从 metadata JSON 中解析。
    pub model: String,
    pub session_started_at: std::time::SystemTime,
}
```

**MemoryFact / MemoryQuery 删除**。这两个类型是过度抽象，外挂系统自行决定内部表示。

**影响文件**：
- `crates/agent-core/src/memory/store.rs`
- `crates/agent-core/src/memory/types.rs`
- `crates/agent-core/src/memory/in_memory.rs`
- `crates/agent-core/src/memory/hook.rs`
- `crates/agent-core/src/memory/extractor.rs`

### 4.3 Conversation Formatter（M2）

新增 `memory/formatter.rs`：

```rust
/// 将一个 turn 的 messages 格式化为 Markdown，供外挂记忆系统消费。
///
/// 输出示例：
/// ```markdown
/// ## Turn 3
///
/// **User**: 帮我重构 src/main.rs
///
/// **ToolCall[read_file]**: path=src/main.rs
///
/// **ToolResult[read_file]**: (成功, 120 行)
/// 内容摘要: fn main() { ... }
///
/// **ToolCall[replace]**: 替换第 15-30 行
///
/// **ToolResult[replace]**: (成功)
///
/// **Assistant**: 我已经重构了 main.rs，主要改动有...
/// ```
pub fn format_turn_content(turn_index: u32, messages: &[AgentMessage]) -> String;

/// 构建结构化元数据，供外挂系统做索引/过滤。
pub fn build_turn_metadata(
    tenant_id: &str,
    session_id: &str,
    turn_index: u32,
    model: &str,
    usage: &Usage,
    stop_reason: &StopReason,
    tool_calls: &[TurnToolCallSummary],
    timestamp: SystemTime,
) -> serde_json::Value;

/// 工具调用摘要（不完整参数，仅 name + 结果状态）
pub struct TurnToolCallSummary {
    pub name: String,
    pub is_error: bool,
    pub result_len: usize,
}
```

**格式化策略**：
- UserMessage：直接输出文本内容
- AssistantMessage with ToolCall：输出 `**ToolCall[name]**` + 关键参数（path, pattern 等）
- ToolResultMessage：输出 `**ToolResult[name]**` + 状态 + 内容摘要（截断至 500 字符）
- AssistantMessage with Text：输出 `**Assistant**` + 完整文本
- Thinking/Image 等 content 类型：跳过或输出占位标记

**影响文件**：
- `crates/agent-core/src/memory/formatter.rs`（新增）
- `crates/agent-core/src/memory/mod.rs`

### 4.4 MemoryHookDispatcher 重写（M4）

```rust
impl HookDispatcher for MemoryHookDispatcher {
    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        let content = format_turn_content(ctx.turn_index, &ctx.messages);
        let tool_summaries: Vec<TurnToolCallSummary> = extract_tool_summaries(&ctx.messages);
        let metadata = build_turn_metadata(
            &ctx.tenant_id, &ctx.session_id,
            ctx.turn_index, &ctx.model,
            &ctx.usage, &ctx.stop_reason,
            &tool_summaries, SystemTime::now(),
        );

        let mem_ctx = MemoryContext { /* ... */ };

        // fire-and-forget to external store
        tokio::spawn(async move {
            match tokio::time::timeout(Duration::from_secs(5), 
                store.remember(&mem_ctx, &content, &metadata)).await
            {
                Ok(Ok(())) => debug!("memory: remembered turn {}", turn_index),
                Ok(Err(e)) => warn!("memory: remember failed: {e}"),
                Err(_) => warn!("memory: remember timed out"),
            }
        });
    }

    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        let query = build_query_string(&ctx.messages);  // 从用户消息构建查询
        if query.is_empty() { return ContextMutation::default(); }

        let facts = match tokio::time::timeout(Duration::from_secs(3),
            store.recall(&mem_ctx, &query)).await
        {
            Ok(Ok(facts)) if !facts.is_empty() => facts,
            _ => return ContextMutation::default(),
        };

        // 将检索结果注入上下文
        let memory_text = facts.join("\n---\n");
        let memory_msg = AgentMessage::User(UserMessage {
            content: vec![Content::Text { 
                text: format!("[Memory]\n{}", memory_text), 
                text_signature: None 
            }],
            timestamp: SystemTime::now(),
        });
        let mut messages = ctx.messages.clone();
        messages.insert(0, memory_msg);
        ContextMutation { messages: Some(messages) }
    }
}
```

**影响文件**：
- `crates/agent-core/src/memory/hook.rs`
- `crates/agent-core/src/memory/extractor.rs`（简化为 query builder + tool summary extractor）

### 4.5 InMemoryStore 适配（M5）

```rust
pub struct InMemoryStore {
    // entity_key → Vec<(content, metadata, timestamp)>
    data: RwLock<HashMap<String, Vec<MemoryRecord>>>,
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn remember(&self, ctx: &MemoryContext, content: &str, metadata: &Value) -> Result<(), MemoryError> {
        let key = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        self.data.write().await.entry(key).or_default().push(MemoryRecord {
            content: content.to_string(),
            metadata: metadata.clone(),
            timestamp: SystemTime::now(),
        });
        Ok(())
    }

    async fn recall(&self, ctx: &MemoryContext, query: &str) -> Result<Vec<String>, MemoryError> {
        let prefix = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        let results: Vec<String> = self.data.read().await
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .flat_map(|(_, records)| records.iter())
            .filter(|r| r.content.contains(query))
            .map(|r| r.content.clone())
            .take(5)
            .collect();
        Ok(results)
    }

    async fn forget_session(&self, ctx: &MemoryContext) -> Result<(), MemoryError> {
        let key = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        self.data.write().await.remove(&key);
        Ok(())
    }
}
```

---

## 5. E2E 测试矩阵

### 5.1 测试基础设施增强

#### 共享 test fixture（I1）

新增 `api-gateway/tests/e2e/common.rs` 中的工厂函数：

```rust
/// 构建带 PG + Redis store 的测试 app
pub async fn build_test_app_with_persistence(
    provider: Arc<dyn LlmProvider>,
) -> (Router, PgSessionStore, RedisSessionStore) {
    let pg_store = create_test_pg_store().await;
    let redis_store = create_test_redis_store().await;
    let config = build_harness_config_with_store(provider, pg_store.clone());
    let app = build_test_app_from_config(config);
    (app, pg_store, redis_store)
}
```

#### Docker 健康检查（I2）

测试启动时验证容器可用：

```rust
pub async fn ensure_test_containers() {
    let pg_ok = sqlx::PgPool::connect(&pg_url()).await.is_ok();
    let redis_ok = redis::Client::open(redis_url())
        .and_then(|c| c.get_connection()).is_ok();
    if !pg_ok || !redis_ok {
        panic!("请先启动 Docker 容器:\n  docker start docker-env-postgres docker-env-redis");
    }
}
```

### 5.2 测试用例

#### E1: `e2e_persistence_recovery` — 全链路持久化恢复

```
GIVEN  session 通过 HTTP API 创建，发送一轮 prompt
WHEN   模拟「服务重启」: flush → 丢弃 TenantManager → 用同一 store 重建
THEN   新 session actor 自动恢复历史，messages() 包含之前的 turn
```

**验证点**：
- 消息历史完整（user + assistant）
- SessionEntry id 保持一致
- Compaction 边界正确恢复

#### E2: `e2e_persistence_compaction` — 压缩 + 持久化组合

```
GIVEN  session 配置低 compaction 阈值（如 5 条消息）
WHEN   连续发送多轮 prompt 触发自动压缩
THEN   entries 中包含 Compaction 类型的 SessionEntry
      压缩后恢复的上下文包含摘要 + 最近消息
```

**验证点**：
- Compaction SessionEntry 正确序列化/反序列化
- CompactionDetails（read_files, modified_files）完整
- 压缩后 token 数显著减少

#### E3: `e2e_persistence_fault_injection` — DB 不可用降级

```
GIVEN  session 配置了 store，但使用无效连接串（如 localhost:19999）
WHEN   发送 prompt
THEN   agent loop 正常完成（持久化为 fire-and-forget，不阻塞）
      日志中出现 "failed to persist session" 警告
      消息历史在内存中完整
```

**实现方式**：使用不存在的端口构造 PG/Redis store（如 `postgres://localhost:19999/postgres`），模拟 DB 不可用场景。测试仅需验证 agent loop 不因持久化失败而中断，不需要真正起停 DB。

**验证点**：
- 持久化失败不传播到 agent loop
- 日志中有明确的 warn 记录
- prompt() 返回正确的 AgentMessage

#### E4: `e2e_concurrent_sessions` — 并发隔离

```
GIVEN  同一租户创建 5 个并发 session
WHEN   每个 session 同时发送不同的 prompt
THEN   每个 session 独立完成，消息不串扰
      SSE 事件流正确隔离（每个 session 只收到自己的事件）
      并发配额正确（第 6 个 session 创建被拒绝）
```

**验证点**：
- 5 个 session 的消息完全隔离
- SSE 事件无交叉
- 配额限流正确

#### E5: `e2e_memory_store` — MemoryStore 联动

```
GIVEN  session 配置了 MemoryStore（InMemoryStore）
WHEN   完成一轮 agent turn
THEN   MemoryHookDispatcher::on_turn_end() 触发 remember()
      content 为正确格式化的 Markdown
      metadata 包含 turn_index, model, tool_calls 等字段
WHEN   删除 session
THEN   MemoryStore.forget_session() 被调用，数据被清理
```

**验证点**：
- format_turn_content() 输出符合预期格式
- build_turn_metadata() 包含所有声明字段
- forget_session 联动正确

---

## 6. 影响范围汇总

### 6.1 crate 变更

| Crate | 变更类型 | 影响 |
|---|---|---|
| `agent-core` | `persistence/store.rs` 新增 `append_entries` | SessionStore trait 扩展 |
| `agent-core` | `harness/session.rs` 自动 restore + 增量保存 | 核心逻辑变更 |
| `agent-core` | `memory/store.rs` trait 简化 | 破坏性 API 变更 |
| `agent-core` | `memory/types.rs` 删除 MemoryFact/MemoryQuery | 破坏性变更 |
| `agent-core` | `memory/formatter.rs` 新增 | 新模块 |
| `agent-core` | `memory/hook.rs` 重写 | MemoryHookDispatcher 对齐新接口 |
| `agent-core` | `memory/in_memory.rs` 适配 | InMemoryStore 对齐新 trait |
| `agent-core` | `memory/extractor.rs` 简化为 query builder | 删除 fact extraction |
| `storage` | `session/postgres.rs` jsonb_insert 增量 | PG 适配器优化 |
| `storage` | `session/redis.rs` append 实现 | Redis 适配器对齐 |
| `tenant` | `manager.rs` MemoryStore forget 联动 | 清理逻辑 |
| `api-gateway` | `tests/e2e/common.rs` 新增 store 注入 | 测试基础设施 |

### 6.2 破坏性变更

- **MemoryStore trait**：`remember(ctx, facts)` → `remember(ctx, content, metadata)`
  - `MemoryFact`、`MemoryQuery` 类型删除
  - 如有外部 `MemoryStore` 实现者需同步更新
- **SessionStore trait 新增默认方法**：`append_entries()` 有默认实现，非破坏性

### 6.3 不涉及

- `SessionActor::new()` 签名不变（自动 restore 为内部行为）
- `HookDispatcher` trait 不变
- TUI 不变
- ai-provider 不变

---

## 7. 测试策略

### 7.1 单元测试

| 模块 | 测试内容 |
|---|---|
| `memory/formatter.rs` | `format_turn_content()` 对各类消息的输出格式验证 |
| `memory/formatter.rs` | `build_turn_metadata()` JSON 结构完整性 |
| `memory/in_memory.rs` | CRUD 操作 |
| `memory/hook.rs` | `on_turn_end` / `on_context` / `on_compact_end` 行为验证 |
| `harness/session.rs` | 自动 restore 行为（有/无 store/空 store） |
| `harness/session.rs` | 增量保存 entry count 正确性 |

### 7.2 集成测试（storage crate）

- 现有 13 个测试保持不变，新增：
- `test_pg_append_entries` — 增量追加
- `test_redis_append_entries` — 增量追加

### 7.3 E2E 测试（api-gateway crate）

- 现有 9 个测试文件保持不变，新增 5 个：
  - `e2e_persistence_recovery.rs`
  - `e2e_persistence_compaction.rs`
  - `e2e_persistence_fault_injection.rs`
  - `e2e_concurrent_sessions.rs`
  - `e2e_memory_store.rs`

### 7.4 运行命令

```bash
# Docker 容器必须先启动
docker start docker-env-postgres docker-env-redis

# 全部 E2E 测试
PANDARIA_TEST_PG_URL="postgres://postgres:postgres@localhost:15432/postgres" \
PANDARIA_TEST_REDIS_URL="redis://:redis@localhost:16379" \
cargo test -p api-gateway --test e2e_session_lifecycle \
  --test e2e_session_state --test e2e_sse_events \
  --test e2e_tenant_isolation --test e2e_tool_use_http \
  --test e2e_webhook --test e2e_websocket --test e2e_api_extensions \
  --test e2e_persistence_recovery --test e2e_persistence_compaction \
  --test e2e_persistence_fault_injection --test e2e_concurrent_sessions \
  --test e2e_memory_store \
  -- --test-threads=1 --nocapture

# 仅持久化相关 E2E
cargo test -p api-gateway --test e2e_persistence_recovery \
  --test e2e_persistence_compaction \
  --test e2e_persistence_fault_injection -- --test-threads=1
```

---

## 8. 风险与缓解

| 风险 | 缓解 |
|---|---|
| MemoryStore trait 破坏性变更影响外部实现者 | 目前无外部实现者（仅 InMemoryStore），直接在仓库内同步更新 |
| 自动 restore 的懒加载策略与现有调用方冲突 | 保留 `restore()` 公开方法为 no-op / 显式 reload 入口；自动恢复在首次 prompt 入口执行 |
| PG jsonb_insert 性能在大 session 下不明确 | 保留全量 save_session 为 fallback；append_entries 默认实现走 load→merge→save |
| Emerald 目前未启动，E2E 测试无法真实调其 API | M5 测试仅验证 Pandaria 侧数据格式正确性（InMemoryStore），Emerald 集成测试单独进行 |


---

## 9. 实现顺序

各项之间存在依赖关系，必须按以下阶段顺序实施：

### Phase 1: Memory 数据准备重构（破坏性变更优先，无外部依赖）

| 顺序 | 项目 | 依赖 | 说明 |
|---|---|---|---|
| 1 | M1 | 无 | MemoryStore trait 简化；删除 MemoryFact / MemoryQuery |
| 2 | M2 | M1 | 新增 `memory/formatter.rs`（纯函数，独立可测） |
| 3 | M3 | M1 | 重写 `memory/extractor.rs` 为 query builder + tool summary |
| 4 | M4 | M1, M2, M3 | 重写 `MemoryHookDispatcher` 对齐新接口 |
| 5 | M5 | M1 | InMemoryStore 适配新 trait |

**Phase 1 检查点**：`cargo test -p agent-core --lib` 全部通过。

### Phase 2: 持久化加固（依赖 Phase 1 的 trait 稳定）

| 顺序 | 项目 | 依赖 | 说明 |
|---|---|---|---|
| 6 | P3 | 无 | `append_entries` trait 方法新增（默认实现，非破坏性） |
| 7 | P3 (PG) | P3 | PG 适配器 override `append_entries` 为 `jsonb_insert` |
| 8 | P3 (Redis) | P3 | Redis 适配器确认默认实现足够 |
| 9 | P2 | 无 | 自动 restore + `restore()` deprecated |
| 10 | P4 | M1 | MemoryStore forget 联动（`tenant/manager.rs`） |

**Phase 2 检查点**：`cargo test -p storage -- --test-threads=1` 全部通过。

### Phase 3: E2E 测试基础设施 + 测试编写

| 顺序 | 项目 | 依赖 | 说明 |
|---|---|---|---|
| 11 | P1 | Phase 1-2 完成 | HarnessConfig 注入 store 到 api-gateway 测试 |
| 12 | I1 | P1 | 共享 test fixture（`build_test_app_with_persistence`） |
| 13 | I2 | 无 | Docker 健康检查前置 |
| 14 | E1 | I1, P2 | 全链路持久化恢复测试 |
| 15 | E2 | I1 | compaction + persistence 组合测试 |
| 16 | E3 | I1 | 故障注入测试 |
| 17 | E4 | I1 | 并发隔离测试 |
| 18 | E5 | I1, M5 | MemoryStore 联动测试 |

**Phase 3 检查点**：全部 14 个 E2E 测试通过。

### 可并行项

- M2（formatter）和 M3（extractor 简化）可并行开发（互不依赖）
- P2（自动 restore）和 P3（增量保存）可并行开发
- E1-E5 可并行编写（共用 I1 fixture）
- I2（Docker 健康检查）可与 Phase 1 同步进行
