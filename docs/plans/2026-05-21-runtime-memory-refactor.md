# Runtime & Memory 架构重构实施计划

**Date:** 2026-05-21
**Status:** Draft
**Reference:** `docs/specs/2026-05-21-runtime-memory-refactor.md`

---

## 1. 概述

本计划将 Runtime & Memory 架构重构 spec 展开为可执行的开发任务。按 Phase 1（Runtime 子模块 + Tenant 瘦身）→ Phase 2（Memory 协议 + Hook 集成）顺序实施，每 Phase 编译通过、测试通过后再进入下一 Phase。

**实施原则**：
- 每 Phase 独立可交付
- 优先保证 `agent-core` 稳定性，`api-gateway` 变更后置
- 所有破坏性变更（`TenantManagerImpl::new` 签名变更）在单 Phase 内完成，不跨 Phase 遗留中间状态

---

## 2. Phase 1：Runtime 子模块 + Tenant 瘦身

### 2.1 目标

将 `TenantManagerImpl::create_session()` 中的 session 组装逻辑下沉到 `agent-core::runtime`，`tenant` 层只负责配额和生命周期管理。本 Phase 不引入 memory 相关代码。

### 2.2 涉及文件

#### 新增文件

| 文件 | 说明 |
|------|------|
| `crates/agent-core/src/runtime/mod.rs` | Runtime 模块入口，导出 `RuntimeConfig`、`SessionBuilder`、`BuiltSession` |
| `crates/agent-core/src/runtime/config.rs` | `RuntimeConfig`、`DefaultHookConfig` 定义 |
| `crates/agent-core/src/runtime/builder.rs` | `SessionBuilder::build()` 实现 |
| `crates/agent-core/src/hook/combined.rs` | `CombinedDispatcher` 实现 |

#### 修改文件

| 文件 | 变更 |
|------|------|
| `crates/agent-core/src/lib.rs` | 导出 `runtime`、`hook::combined` |
| `crates/agent-core/src/hook/mod.rs` | 导出 `CombinedDispatcher` |
| `crates/agent-core/src/harness/session.rs` | 新增 `pub fn tools(&self) -> &[AgentToolRef]` |
| `crates/tenant/src/manager.rs` | `TenantManagerImpl` 字段精简 + `new()` 签名变更 + `create_session()` 使用 `SessionBuilder` + `delete_session()` 增加 `forget_session` 占位（Phase 1 中 `memory_store` 始终为 `None`，调用无实际效果） |
| `crates/api-gateway/src/main.rs`（或启动文件） | 重构启动代码，先构造 `RuntimeConfig` 再传入 `TenantManagerImpl::new()` |

#### 删除/废弃

| 内容 | 说明 |
|------|------|
| `TenantManagerImpl::with_media()` | 移除，配置迁移到 `RuntimeConfig` |
| `TenantManagerImpl::with_media()` | 移除，配置迁移到 `RuntimeConfig` |
| `TenantManagerImpl::with_cost_tracker()` | 移除，`cost_callback` 通过 `DefaultHookConfig` 注入 |
| `TenantManagerImpl::with_http_client()` | 移除，配置迁移到 `RuntimeConfig` |
| `TenantManagerImpl::with_available_models()` | 移除，配置迁移到 `RuntimeConfig` |
| `TenantManagerImpl::with_max_sync_wait_ms()` | 移除，直接作为 `TenantManagerImpl` 字段保留 |
| `TenantManagerImpl::with_http_client()` | 移除，配置迁移到 `RuntimeConfig` |
| `TenantManagerImpl::with_available_models()` | 移除，配置迁移到 `RuntimeConfig` |
| `TenantManagerImpl::with_max_sync_wait_ms()` | 移除，配置迁移到 `TenantManagerImpl` 直接字段 |

### 2.3 具体步骤

#### Step 1.1：创建 `agent-core/src/hook/combined.rs`

实现 `CombinedDispatcher`：

```rust
pub struct CombinedDispatcher {
    chain: Vec<Arc<dyn HookDispatcher>>,
}

impl CombinedDispatcher {
    pub fn new(chain: Vec<Arc<dyn HookDispatcher>>) -> Self {
        Self { chain }
    }
}

#[async_trait]
impl HookDispatcher for CombinedDispatcher {
    // 阻塞型 hook：first-block-wins
    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        for d in &self.chain {
            let (decision, mutation) = d.on_tool_call(ctx).await;
            if matches!(decision, HookDecision::Block { .. }) {
                return (decision, mutation);
            }
        }
        (HookDecision::Continue, ToolCallMutation::default())
    }

    async fn on_before_compact(&self, ctx: &CompactCtx) -> CompactDecision {
        for d in &self.chain {
            let decision = d.on_before_compact(ctx).await;
            if matches!(decision, CompactDecision::Block { .. } | CompactDecision::Replace { .. }) {
                return decision;
            }
        }
        CompactDecision::Continue
    }

    // 链式 hook：管道模式
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

    async fn on_before_agent_start(&self, ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
        let mut mutation = BeforeAgentStartMutation::default();
        for d in &self.chain {
            let next = d.on_before_agent_start(&BeforeAgentStartCtx {
                tenant_id: ctx.tenant_id.clone(),
                session_id: ctx.session_id.clone(),
                system_prompt: mutation.system_prompt.as_ref().or(ctx.system_prompt.as_ref()).cloned(),
                prompt_builder: mutation.system_prompt.clone().unwrap_or_else(|| ctx.prompt_builder.clone()),
                messages: mutation.messages.clone().unwrap_or_else(|| ctx.messages.clone()),
                tools: ctx.tools.clone(),
                model: ctx.model.clone(),
            }).await;
            if next.system_prompt.is_some() { mutation.system_prompt = next.system_prompt; }
            if next.prompt_mutation.is_some() { mutation.prompt_mutation = next.prompt_mutation; }
            if next.messages.is_some() { mutation.messages = next.messages; }
        }
        mutation
    }

    async fn on_before_provider_request(&self, ctx: &ProviderRequestCtx) -> ProviderRequestMutation {
        let mut mutation = ProviderRequestMutation::default();
        for d in &self.chain {
            let next = d.on_before_provider_request(&ProviderRequestCtx {
                tenant_id: ctx.tenant_id.clone(),
                session_id: ctx.session_id.clone(),
                model: ctx.model.clone(),
                system_prompt: mutation.system_prompt.as_ref().or(ctx.system_prompt.as_ref()).cloned(),
                prompt_builder: mutation.system_prompt.clone().unwrap_or_else(|| ctx.prompt_builder.clone()),
                messages: mutation.messages.clone().unwrap_or_else(|| ctx.messages.clone()),
                turn_index: ctx.turn_index,
                tools: mutation.tools.unwrap_or_else(|| ctx.tools.clone()),
                options: mutation.options.clone().unwrap_or_else(|| ctx.options.clone()),
            }).await;
            if next.system_prompt.is_some() { mutation.system_prompt = next.system_prompt; }
            if next.prompt_mutation.is_some() { mutation.prompt_mutation = next.prompt_mutation; }
            if next.messages.is_some() { mutation.messages = next.messages; }
            if next.tools.is_some() { mutation.tools = next.tools; }
            if next.options.is_some() { mutation.options = next.options; }
        }
        mutation
    }

    async fn on_after_provider_response(&self, ctx: &ProviderResponseCtx) -> ProviderResponseMutation {
        let mut mutation = ProviderResponseMutation::default();
        for d in &self.chain {
            let next = d.on_after_provider_response(&ProviderResponseCtx {
                tenant_id: ctx.tenant_id.clone(),
                session_id: ctx.session_id.clone(),
                model: ctx.model.clone(),
                content: mutation.content.clone().unwrap_or_else(|| ctx.content.clone()),
                turn_index: ctx.turn_index,
                attempt: ctx.attempt,
                messages_before: ctx.messages_before.clone(),
                stop_reason: mutation.stop_reason.unwrap_or(ctx.stop_reason),
            }).await;
            if next.content.is_some() { mutation.content = next.content; }
            if next.stop_reason.is_some() { mutation.stop_reason = next.stop_reason; }
        }
        mutation
    }

    // 观测型 hook：fire-and-forget，顺序执行
    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        for d in &self.chain { d.on_turn_end(ctx).await; }
    }
    async fn on_agent_end(&self, ctx: &AgentEndCtx) {
        for d in &self.chain { d.on_agent_end(ctx).await; }
    }
    async fn on_session_start(&self, ctx: &SessionCtx) {
        for d in &self.chain { d.on_session_start(ctx).await; }
    }
    async fn on_tool_execution_start(&self, ctx: &ToolExecutionStartCtx) {
        for d in &self.chain { d.on_tool_execution_start(ctx).await; }
    }
    async fn on_tool_execution_end(&self, ctx: &ToolExecutionEndCtx) {
        for d in &self.chain { d.on_tool_execution_end(ctx).await; }
    }
    async fn on_compact_end(&self, ctx: &CompactEndCtx) {
        for d in &self.chain { d.on_compact_end(ctx).await; }
    }
}
```

**测试要求**：
- 测试 `CombinedDispatcher` 的阻塞型 hook first-block-wins 语义
- 测试 `on_context` 管道模式（两个子 dispatcher 依次修改 messages）
- 测试空 chain（所有 hook 返回 default）

#### Step 1.2：创建 `agent-core/src/runtime/config.rs`

```rust
use std::collections::HashMap;
use std::sync::Arc;

/// DefaultHookDispatcher 的可配置策略字段
#[derive(Debug, Clone)]
pub struct DefaultHookConfig {
    pub denied_tools: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub path_guard_fields: HashMap<String, Vec<String>>,
    pub path_guard_scan_unknown: bool,
    pub max_turns_per_session: usize,
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

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub provider: Arc<dyn ai_provider::LlmProvider>,
    pub default_model: String,
    pub default_system_prompt: String,
    pub default_context_window: usize,
    pub store: Option<Arc<dyn crate::persistence::store::SessionStore>>,
    pub media_provider: Option<Arc<dyn ai_provider::MediaProvider>>,
    pub media_registry: Option<Arc<ai_provider::MediaModelRegistry>>,
    pub http_client: reqwest::Client,
    pub compaction_config: crate::harness::compaction::CompactionConfig,
    pub agent_space: crate::space::AgentSpace,
    pub hook_config: DefaultHookConfig,
    /// Phase 1 中始终为 None，Phase 2 再传入实际实现
    pub memory_store: Option<Arc<dyn crate::memory::MemoryStore>>,
}
```

**注意**：Phase 1 中 `memory_store` 始终为 `None`，结构体本身在 Phase 1 就完整定义，避免两 Phase 之间结构体变化。

#### Step 1.3：创建 `agent-core/src/runtime/builder.rs`

实现 `SessionBuilder` 和 `BuiltSession`。关键逻辑：

1. 从 `RuntimeConfig` 读取配置创建 `DefaultHookDispatcher`
2. 创建 tools 列表（media tool + http proxy tools）
3. 创建 `CompactionActor`
4. 加载 skills（复用现有 `FileSystemSkillLoader` 逻辑，从 `agent_space` 推导路径）
5. 组装 `SessionConfig`，调用 `SessionActor::new()`
6. 返回 `BuiltSession { actor, tools }`

**注意**：skills 加载是 async 的，`load_skills()` 需要 `&self`（通过 `tenant_id` 推导 project_skills_dir）。

#### Step 1.4：修改 `crates/agent-core/src/harness/session.rs`

新增 `tools()` getter：

```rust
impl SessionActor {
    pub fn tools(&self) -> &[crate::types::AgentToolRef] {
        &self.tools
    }
}
```

#### Step 1.5：修改 `crates/tenant/src/manager.rs`

1. 精简 `TenantManagerImpl` 字段，只保留 `registry`、`runtime_config`、`sessions`、`available_models`、`max_sync_wait_ms`
2. `new()` 签名改为接收 `Arc<RuntimeConfig>`
3. `create_session()` 使用 `SessionBuilder::new(&self.runtime_config)` 创建 session
4. `delete_session()` 中，在 `actor.shutdown()` 之后增加 `forget_session` 调用（Phase 1 中 `memory_store` 为 `None`，此调用无实际效果）
5. 移除所有 `with_*` builder 方法

**注意**：`ActiveSession` 的 `tools` 字段需要从 `BuiltSession.tools` 填充。`original_tools` 仍然来自 `params.tools`。

#### Step 1.6：修改 `crates/api-gateway/src/main.rs`（或等效启动文件）

重构启动代码：

```rust
// 构造 RuntimeConfig
let runtime_config = Arc::new(agent_core::RuntimeConfig {
    provider: provider.clone(),
    default_model: config.default_model.clone(),
    default_system_prompt: config.default_system_prompt.clone(),
    default_context_window: config.default_context_window,
    store: Some(store.clone()),
    media_provider: media_provider.clone(),
    media_registry: media_registry.clone(),
    http_client: http_client.clone(),
    compaction_config: agent_core::CompactionConfig {
        enabled: true,
        reserve_tokens: 4096,
        keep_recent_tokens: 8192,
    },
    agent_space: AgentSpace::from_env_or_default(),
    hook_config: DefaultHookConfig {
        cost_callback: Some(Arc::new(move |tenant_id, cost| {
            cost_tracker.record_media_call(cost);
        })),
        ..Default::default()
    },
});

let tenant_manager = Arc::new(TenantManagerImpl::new(registry, runtime_config));
```

#### Step 1.7：编译与测试

```bash
cargo check --workspace
cargo test --workspace
```

### 2.4 验收标准

- [ ] `cargo check --workspace` 无编译错误
- [ ] `cargo test --workspace` 全部通过
- [ ] `TenantManagerImpl` 的 `with_*` builder 方法全部移除
- [ ] `TenantManagerImpl::new()` 只接收 `registry` + `runtime_config`
- [ ] `create_session()` 代码行数从 ~170 缩减到 ~40 以内
- [ ] `CombinedDispatcher` 有独立单元测试覆盖阻塞型和链式 hook 语义

---

## 3. Phase 2：Memory 协议 + Hook 集成

### 3.1 目标

在 `agent-core` 中引入 `memory/` 子模块，定义 `MemoryStore` trait 和 `MemoryHookDispatcher`，并集成到 `SessionBuilder` 中。

### 3.2 涉及文件

#### 新增文件

| 文件 | 说明 |
|------|------|
| `crates/agent-core/src/memory/mod.rs` | Memory 模块入口 |
| `crates/agent-core/src/memory/store.rs` | `MemoryStore` trait + `MemoryError` |
| `crates/agent-core/src/memory/types.rs` | `MemoryContext`、`MemoryFact`、`MemoryQuery`、`MemoryCategory` |
| `crates/agent-core/src/memory/hook.rs` | `MemoryHookDispatcher` |
| `crates/agent-core/src/memory/extractor.rs` | `extract_facts`、`build_query`、`format_facts` |
| `crates/agent-core/src/memory/in_memory.rs` | `InMemoryStore`（纯内存实现，仅用于测试） |

#### 修改文件

| 文件 | 变更 |
|------|------|
| `crates/agent-core/src/lib.rs` | 导出 `memory` |
| `crates/agent-core/src/runtime/config.rs` | `RuntimeConfig` 新增 `memory_store: Option<Arc<dyn MemoryStore>>` |
| `crates/agent-core/src/runtime/builder.rs` | `build()` 中判断 `memory_store` 是否存在，若存在则组合 `MemoryHookDispatcher` |
| `crates/agent-core/src/hook/context.rs` | `CompactEndCtx` 新增 `result: Option<crate::compaction::CompactionResult>` |
| `crates/agent-core/src/harness/session.rs` | `run_auto_compaction()` 中 compaction 完成后触发 `on_compact_end` hook |
| `crates/tenant/src/manager.rs` | `delete_session()` 中 `forget_session` 调用从占位变为实际生效（因为 `memory_store` 可能已配置） |

### 3.3 具体步骤

#### Step 2.1：创建 `memory/types.rs`

```rust
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct MemoryContext {
    pub tenant_id: String,
    pub session_id: String,
    pub user_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemoryFact {
    pub id: Option<String>,
    pub content: String,
    pub category: Option<String>,
    pub importance: Option<u8>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct MemoryQuery {
    pub text: String,
    pub limit: usize,
    pub session_only: bool,
}
```

#### Step 2.2：创建 `memory/store.rs`

```rust
use async_trait::async_trait;

#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn remember(&self, ctx: &MemoryContext, facts: &[MemoryFact]) -> Result<(), MemoryError>;
    async fn recall(&self, ctx: &MemoryContext, query: &MemoryQuery) -> Result<Vec<MemoryFact>, MemoryError>;
    async fn forget_session(&self, ctx: &MemoryContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("memory store error: {0}")]
    StoreError(String),
}
```

#### Step 2.3：创建 `memory/extractor.rs`

实现 `extract_facts`、`build_query`、`format_facts`：

```rust
/// 从一轮对话中提取关键事实
pub fn extract_facts(messages: &[AgentMessage]) -> Vec<MemoryFact> {
    let mut facts = Vec::new();
    for msg in messages {
        match msg {
            AgentMessage::Assistant(a) => {
                // 只提取最终回复（不含 tool calls 的文本内容）
                let text: String = a.content.iter()
                    .filter_map(|c| match c {
                        ai_provider::Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() && text.len() > 20 {
                    facts.push(MemoryFact {
                        id: None,
                        content: text,
                        category: Some("assistant_response".to_string()),
                        importance: Some(5),
                        metadata: serde_json::Value::Null,
                    });
                }
            }
            AgentMessage::ToolResult(tr) if !tr.is_error => {
                // 提取重要 tool results
                let text: String = tr.content.iter()
                    .filter_map(|c| match c {
                        ai_provider::Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() && text.len() > 10 {
                    facts.push(MemoryFact {
                        id: None,
                        content: format!("[Tool: {}] {}", tr.tool_name, text),
                        category: Some("tool_result".to_string()),
                        importance: Some(4),
                        metadata: serde_json::json!({"tool_name": tr.tool_name}),
                    });
                }
            }
            _ => {}
        }
    }
    facts
}

/// 用最近 1-2 轮用户消息构建检索查询
pub fn build_query(messages: &[AgentMessage]) -> MemoryQuery {
    let recent_user_text: Vec<String> = messages.iter().rev().take(3).filter_map(|m| {
        if let AgentMessage::User(u) = m {
            Some(u.content.iter().filter_map(|c| match c {
                ai_provider::Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            }).collect::<Vec<_>>().join(" "))
        } else { None }
    }).collect();

    MemoryQuery {
        text: recent_user_text.join("\n"),
        limit: 5,
        session_only: false,
    }
}

/// 将检索到的事实格式化为提示文本
pub fn format_facts(facts: &[MemoryFact]) -> String {
    facts.iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n---\n")
}
```

**注意**：以上提取策略为 MVP 实现，后续可根据实际效果调优。

#### Step 2.4：创建 `memory/hook.rs`

实现 `MemoryHookDispatcher`，完整实现 `on_turn_end`、`on_context`、`on_compact_end`。

#### Step 2.5：创建 `memory/in_memory.rs`

```rust
use std::collections::HashMap;
use std::sync::Mutex;

pub struct InMemoryStore {
    data: Mutex<HashMap<String, Vec<MemoryFact>>>, // key = "tenant_id:session_id"
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn remember(&self, ctx: &MemoryContext, facts: &[MemoryFact]) -> Result<(), MemoryError> {
        let mut data = self.data.lock().unwrap();
        let key = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        data.entry(key).or_default().extend(facts.iter().cloned());
        Ok(())
    }

    async fn recall(&self, ctx: &MemoryContext, query: &MemoryQuery) -> Result<Vec<MemoryFact>, MemoryError> {
        let data = self.data.lock().unwrap();
        let prefix = if query.session_only {
            format!("{}:{}", ctx.tenant_id, ctx.session_id)
        } else {
            format!("{}:", ctx.tenant_id)
        };
        let mut results: Vec<MemoryFact> = data.iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .flat_map(|(_, v)| v.clone())
            .filter(|f| f.content.contains(&query.text)) // 简单文本匹配
            .take(query.limit)
            .collect();
        Ok(results)
    }

    async fn forget_session(&self, ctx: &MemoryContext) -> Result<(), MemoryError> {
        let mut data = self.data.lock().unwrap();
        let key = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        data.remove(&key);
        Ok(())
    }
}
```

#### Step 2.6：修改 `RuntimeConfig` 和 `SessionBuilder`

- `RuntimeConfig` 新增 `memory_store: Option<Arc<dyn MemoryStore>>`
- `SessionBuilder::build()` 中判断 `memory_store`：
  - 若为 `Some`，创建 `CombinedDispatcher::new(vec![base, MemoryHookDispatcher::new(mem)])`
  - 若为 `None`，直接使用 `base`

#### Step 2.7：修改 `CompactEndCtx` 和 compaction 触发点

1. `CompactEndCtx` 新增 `result: Option<crate::compaction::CompactionResult>`
2. `SessionActor::run_auto_compaction()` 中，compaction 完成后构造 `CompactEndCtx` 并调用 `hook_dispatcher.on_compact_end(&ctx)`

#### Step 2.8：编译与测试

```bash
cargo check --workspace
cargo test -p agent-core
cargo test --workspace
```

### 3.4 验收标准

- [ ] `cargo check --workspace` 无编译错误
- [ ] `cargo test -p agent-core` 全部通过，包含 `MemoryHookDispatcher` 单元测试和 `InMemoryStore` 集成测试
- [ ] `cargo test --workspace` 全部通过
- [ ] `MemoryStore` trait 有清晰的文档注释
- [ ] `MemoryHookDispatcher` 的 `on_turn_end` 和 `on_context` 有独立单元测试
- [ ] `on_compact_end` 触发逻辑有测试覆盖

---

## 4. 测试策略

### 4.1 单元测试

| 测试目标 | 位置 | 覆盖内容 |
|---------|------|---------|
| `CombinedDispatcher` | `agent-core/src/hook/combined.rs` 的 `#[cfg(test)]` | 阻塞型 first-block-wins、链式管道模式、空 chain |
| `SessionBuilder` | `agent-core/src/runtime/builder.rs` 的 `#[cfg(test)]` | 带/不带 memory_store 的构建、tools 列表正确性 |
| `MemoryHookDispatcher` | `agent-core/src/memory/hook.rs` 的 `#[cfg(test)]` | `on_turn_end` 提取 + 写入、`on_context` 检索 + 注入、`on_compact_end` 保存 summary |
| `extract_facts` / `build_query` | `agent-core/src/memory/extractor.rs` 的 `#[cfg(test)]` | 各种消息组合的提取结果、查询构建逻辑 |
| `InMemoryStore` | `agent-core/src/memory/in_memory.rs` 的 `#[cfg(test)]` | remember/recall/forget_session 完整 CRUD |

### 4.2 集成测试

- 使用 `InMemoryStore` 作为 `memory_store`，完整跑一个多 turn session，验证：
  - turn 1 的 assistant 回复能在 turn 2 的 `on_context` 中被检索到
  - session 删除后 `forget_session` 被调用，记忆不再被检索到
  - compaction 后 summary 被写入 memory

### 4.3 回归测试

- 运行现有 `agent-core` 全部测试，确保 `SessionActor`、`AgentLoop`、`CompactionActor` 行为不变
- 运行 `tenant` 全部测试，确保 `TenantManagerImpl` 的公开 API 行为不变
- 运行 `storage` 全部测试，确保 `SessionStore` 实现不受影响

---

## 5. 风险与回滚

### 5.1 风险

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| `TenantManagerImpl::new()` 签名变更影响所有测试 | 高 | 在同一 commit 中同步更新所有测试和启动代码 |
| `CombinedDispatcher` 管道模式引入意外的 mutation 叠加 | 中 | 独立单元测试覆盖，Phase 1 完成后再进入 Phase 2 |
| `MemoryHookDispatcher` 的 `remember` fire-and-forget 可能导致 memory 丢失 | 低 | 这是设计决策（避免阻塞 loop），tracing 记录失败事件 |
| `CompactEndCtx` 新增字段是 breaking change | 中 | `#[non_exhaustive]` 已存在，外部构造点使用 `new()` 构造函数，不受影响 |

### 5.2 回滚策略

- Phase 1 和 Phase 2 各自为一个独立的 feature branch
- 每 Phase 完成后 squash merge 到主分支
- 若 Phase 2 发现问题，可单独 revert Phase 2 的 commit，Phase 1 的 runtime 重构不受影响

---

## 6. 验收总结

| 检查项 | Phase 1 | Phase 2 |
|--------|---------|---------|
| 编译通过 | ✅ | ✅ |
| 单元测试通过 | ✅ | ✅ |
| 集成测试通过 | — | ✅ |
| 回归测试通过 | ✅ | ✅ |
| 文档注释完整 | — | ✅ |
| `TenantManagerImpl::create_session()` < 40 行 | ✅ | — |
| `agent-core` 新增模块有 README 级注释 | ✅ | ✅ |
