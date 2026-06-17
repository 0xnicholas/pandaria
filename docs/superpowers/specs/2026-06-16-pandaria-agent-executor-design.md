# PandariaAgentExecutor 设计（v2 — review 修正版）

> 状态：设计已确认（v2），待实现  
> 作者：pi / Nicholas  
> 日期：2026-06-16  
> 修订：v2 基于 code review（15 issues），重写并发模型、agent 解析、tool 转换、flush 语义等章节

---

## 1. 背景

Tavern 的 `AgentExecutor` trait 已定义，`SquadEngine` 支持 DAG / Manager-Worker 编排，但生产级实现 `PandariaAgentExecutor` 尚未落地。`agent-core` 的 `SessionActor` 提供完整 agent runtime（tenant/session 隔离、HookDispatcher、memory/compaction、持久化、token 计量、tracing）。

本设计让 `SquadEngine` 接入 `SessionActor`，使 Agent Team 自动继承 Pandaria 的多租户能力。

---

## 2. 定位

`PandariaAgentExecutor` 是 `AgentExecutor` 的生产级实现：

- 每个 `role_id:model` 组合复用一个 `SessionActor`（cache key 包含 model，避免 model_override 被静默丢弃）。
- 同一 key 的并发调用串行化在 `tokio::sync::Mutex::lock().await` 上，**不引入临时 session 回退机制**。
- 通过 `AgentResolver` trait 解析 `AgentConfig`（解耦 `TavernHero`）。
- `execute_stream` 本次 stub，后续补齐。

---

## 3. 架构

### 3.1 新增/修改文件

| 操作 | 文件 | 说明 |
|---|---|---|
| Create | `crates/tavern-comp/src/team/pandaria_executor.rs` | `PandariaAgentExecutor` 主实现 |
| Modify | `crates/tavern-comp/src/team/mod.rs` | 导出 `pandaria_executor` 模块 |
| Modify | `crates/tavern-comp/src/team/executor.rs` | `AgentInput` 加 `squad_id`, `mission_id`；trait 加 `flush()` |
| Modify | `crates/tavern-comp/src/team/engine.rs` | `SquadEngine` 构造 `AgentInput` 时填入 `squad_id` / `mission_id` |
| Modify | `crates/tavern-comp/src/error.rs` | `AgentExecutorError` 新增 variant |
| Modify | `crates/tavern-comp/src/lib.rs` | 公开导出 `PandariaAgentExecutor` |

### 3.2 `AgentResolver` trait（新增）

定义在 `crates/tavern-comp/src/team/executor.rs`（与 `AgentExecutor` 同文件）：

```rust
/// Resolves an `AgentConfig` from an agent identifier.
/// Implemented by `TavernHero` in production, mockable in tests.
#[async_trait]
pub trait AgentResolver: Send + Sync {
    async fn resolve(&self, agent_id: &str) -> Option<tavern_core::AgentConfig>;
}
```

`TavernHero` 实现此 trait（在 `crates/tavern-comp/src/hero/hero.rs`）：

```rust
#[async_trait]
impl AgentResolver for TavernHero {
    async fn resolve(&self, agent_id: &str) -> Option<AgentConfig> {
        self.get_agent(agent_id).await
    }
}
```

### 3.3 `PandariaAgentExecutor` 结构

```rust
#[derive(Clone)]
pub struct PandariaAgentExecutor {
    tenant_id: String,
    team_id: String,
    harness_config: agent_core::HarnessConfig,
    agent_resolver: Arc<dyn AgentResolver>,
    sessions: Arc<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<SessionActor>>>>>,
    session_semaphore: Arc<tokio::sync::Semaphore>,
}
```

- `team_id`：用于 tracing span。
- `agent_resolver`：替代 `Arc<TavernHero>`，解耦测试。
- `sessions`：key = `"{role_id}:{model}"`。
- `session_semaphore`：限制最大并发 session 数（默认 8），防止内存/配额暴涨。

### 3.4 构造

```rust
impl PandariaAgentExecutor {
    pub fn new(
        tenant_id: impl Into<String>,
        team_id: impl Into<String>,
        harness_config: agent_core::HarnessConfig,
        agent_resolver: Arc<dyn AgentResolver>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            team_id: team_id.into(),
            harness_config,
            agent_resolver,
            sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            session_semaphore: Arc::new(tokio::sync::Semaphore::new(8)),
        }
    }

    pub fn with_max_sessions(mut self, n: usize) -> Self {
        self.session_semaphore = Arc::new(tokio::sync::Semaphore::new(n.max(1)));
        self
    }
}
```

---

## 4. 数据流

### 4.1 `resolve_role`

`PandariaAgentExecutor::resolve_role` 仅从 `AgentConfig` 构建 `Role` 的 agent 级字段。
`team_instructions` 和 `visibility` 等 team 级字段由 `SquadEngine` 从 `team.roles` 合并（见 §9 SquadEngine 改动）。

```rust
async fn resolve_role(&self, role_id: &str) -> Result<Role, AgentExecutorError> {
    let agent = self.agent_resolver
        .resolve(role_id)
        .await
        .ok_or_else(|| AgentExecutorError::RoleNotFound { id: role_id.into() })?;

    Ok(Role {
        id: role_id.into(),
        name: agent.name.clone(),
        description: agent.description.clone(),
        agent_id: agent.id.clone(),
        team_instructions: None,       // SquadEngine 负责合并
        model_override: Some(agent.model.clone()),
        visibility: Default::default(), // SquadEngine 负责合并
        skills: agent.skills.clone(),
    })
}
```

### 4.2 `execute`

```rust
#[tracing::instrument(
    skip(self, input),
    fields(
        tenant_id = %self.tenant_id,
        team_id = %self.team_id,
        role_id = %role_id,
        squad_id = %input.squad_id.as_deref().unwrap_or("unknown"),
        mission_id = %input.mission_id.as_deref().unwrap_or("unknown"),
    )
)]
async fn execute(
    &self,
    role_id: &str,
    input: AgentInput,
) -> Result<AgentOutput, AgentExecutorError> {
    let start = std::time::Instant::now();

    // 1. Resolve agent config
    let agent = self.agent_resolver
        .resolve(role_id)
        .await
        .ok_or_else(|| AgentExecutorError::RoleNotFound { id: role_id.into() })?;

    // 2. Determine model string
    let model = match &input.model_override {
        Some(m) => format!("{}/{}", m.provider, m.name),
        None => format!("{}/{}", agent.model.provider, agent.model.name),
    };

    // 3. Acquire or create cached session (key = role_id:model)
    let session_arc = self.acquire_session(role_id, &model, &agent).await?;

    // 4. Build prompt from AgentInput + TeamContext
    let prompt = build_role_prompt(&input, &agent, role_id);

    // 5. Execute (no internal timeout — timeout is caller responsibility)
    //    SessionActor::complete() is &mut self, so lock is held for
    //    the full duration of the LLM call.
    let mut actor = session_arc.lock().await;
    let text = actor.complete(prompt).await
        .map_err(|e| map_agent_error(e))?;
    drop(actor);

    // 6. Build output (usage left as None for P0; follow-up adds SessionActor::last_usage())
    Ok(AgentOutput {
        content: serde_json::Value::String(text),
        usage: None,
        latency: start.elapsed(),
        metadata: HashMap::new(),
    })
}
```

**Timeout 说明：** `execute` 内部不做 `tokio::time::timeout`。原因是 `SessionActor::complete()` 被 timeout 丢弃后 state 不一致（Running 状态未重置、last_save handle 未 await）。上层（SquadEngine）通过 `Mission.timeout` 字段控制；若需超时，在 SquadEngine 层做 `tokio::time::timeout` 并 discard 该 session（从 cache 移除）。

### 4.3 `acquire_session`

```rust
async fn acquire_session(
    &self,
    role_id: &str,
    model: &str,
    agent: &tavern_core::AgentConfig,
) -> Result<Arc<tokio::sync::Mutex<SessionActor>>, AgentExecutorError> {
    let cache_key = format!("{}:{}", role_id, model);

    // Fast path: check cache
    {
        let map = self.sessions.lock()
            .expect("pandaria executor session map poisoned");
        if let Some(actor_arc) = map.get(&cache_key) {
            return Ok(actor_arc.clone());
        }
    }

    // Slow path: build new session (bounded by semaphore)
    let _permit = self.session_semaphore
        .acquire()
        .await
        .expect("session semaphore should not be closed");

    // Double-check after acquiring semaphore
    {
        let map = self.sessions.lock()
            .expect("pandaria executor session map poisoned");
        if let Some(actor_arc) = map.get(&cache_key) {
            return Ok(actor_arc.clone());
        }
    }

    let session_id = format!("{}-{}-{}", self.tenant_id, role_id, uuid::Uuid::new_v4());

    // Build tools from agent skills (Sidecar runner only for P0)
    let tools = build_tool_configs(&agent.skills);

    let built = agent_core::SessionBuilder::new(&self.harness_config)
        .tenant_id(self.tenant_id.clone())
        .session_id(session_id)
        .system_prompt(agent.instructions.clone())  // raw instructions, no skill injection
        .model(model.to_string())
        .with_external_tools(tools)
        .build()
        .await
        .map_err(|e| AgentExecutorError::SessionBuildFailed {
            reason: e.to_string(),
        })?;

    let actor_arc = Arc::new(tokio::sync::Mutex::new(built.actor));

    {
        let mut map = self.sessions.lock()
            .expect("pandaria executor session map poisoned");
        map.entry(cache_key).or_insert(actor_arc.clone());
    }

    Ok(actor_arc)
}
```

- **Double-check**: 在 semaphore 获取后重新检查 cache，避免重复创建。
- **Skill 注入**: system_prompt 只传 `agent.instructions` 原始文本，skills 由 `SessionBuilder::build()` 内部通过 `PromptBuilder` 统一注入，**不会重复**。

### 4.4 `build_tool_configs`（skill → ToolConfig 转换）

```rust
/// Convert `SkillConfig` list to `agent_core::tools::ToolConfig` list.
/// P0: only Sidecar runner skills are converted to HTTP proxy tools.
/// Rust/subprocess skills are skipped (documented limitation).
fn build_tool_configs(skills: &[tavern_core::SkillConfig]) -> Vec<agent_core::tools::ToolConfig> {
    skills
        .iter()
        .filter(|s| matches!(s.runner, tavern_core::ToolRunner::Sidecar) && s.url.is_some())
        .map(|s| agent_core::tools::ToolConfig {
            name: s.name.clone().unwrap_or_else(|| s.id.clone()),
            description: s.description.clone().unwrap_or_default(),
            parameters: s.parameters.clone(),
            endpoint: s.url.clone().expect("filtered for Some"),
            timeout_ms: Some(s.timeout_ms),
            headers: None,
        })
        .collect()
}
```

**不再依赖 `TAVERN_PUBLIC_URL` / `TAVERN_TOOL_SECRET` 环境变量**（review issue #9）。

### 4.5 `execute_stream` stub

```rust
async fn execute_stream(
    &self,
    _role_id: &str,
    _input: AgentInput,
) -> Result<BoxStream<'static, AgentOutputChunk>, AgentExecutorError> {
    Ok(Box::pin(futures_util::stream::empty()))
}
```

---

## 5. Prompt 构建

### 5.1 `build_role_prompt`

```rust
fn build_role_prompt(input: &AgentInput, agent: &tavern_core::AgentConfig, role_id: &str) -> String {
    let mut parts = Vec::new();

    // 1. Current role's private context
    if let Some(private_val) = input.context.private.get(role_id) {
        parts.push(format!("[Private Context]\n{}", private_val));
    }

    // 2. Shared context
    if !input.context.shared.is_null() {
        parts.push(format!("[Shared Context]\n{}", input.context.shared));
    }

    // 3. Recent thread messages (last 5)
    let recent: Vec<_> = input.context.thread.iter().rev().take(5).collect();
    if !recent.is_empty() {
        let msgs: Vec<String> = recent.iter().rev()
            .map(|m| format!("[{}] {}", m.role, m.content))
            .collect();
        parts.push(format!("[Recent Messages]\n{}", msgs.join("\n")));
    }

    // 4. Task
    parts.push(format!("[Task]\n{}", input.task));

    parts.join("\n\n")
}
```

### 5.2 System prompt

直接传 `agent.instructions`。**不注入 skills**（skills 由 SessionBuilder 通过 PromptBuilder 注入），**不复用 TavernHero 的 `build_system_prompt()`**（避免重复和 TavernHero 耦合）。

### 5.3 Handoff 识别

`AgentOutput.content` 为 `Value::String(text)`，`SquadEngine` 已有 `Handoff::detect(&output.content)` 逻辑，无需 `PandariaAgentExecutor` 额外处理。

---

## 6. 错误处理

### 6.1 `AgentExecutorError` 新增 variant

```rust
#[derive(Debug, Clone, thiserror::Error)]
pub enum AgentExecutorError {
    #[error("role not found: {id}")]
    RoleNotFound { id: String },

    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    #[error("timeout")]
    Timeout,

    // ── v2 新增 ──
    #[error("session build failed: {reason}")]
    SessionBuildFailed { reason: String },

    #[error("provider error: {0}")]
    ProviderError(String),

    #[error("tool denied: {tool} — {reason}")]
    ToolDenied { tool: String, reason: String },

    #[error("context overflow: {0}")]
    ContextOverflow(String),
}
```

### 6.2 `map_agent_error` 辅助函数

```rust
fn map_agent_error(e: agent_core::error::AgentError) -> AgentExecutorError {
    use agent_core::error::AgentError;

    match &e {
        AgentError::ToolDenied { tool, reason } => AgentExecutorError::ToolDenied {
            tool: tool.clone(),
            reason: reason.clone(),
        },
        AgentError::ContextOverflow(msg) => AgentExecutorError::ContextOverflow(msg.clone()),
        // Remaining errors → ExecutionFailed
        _ => AgentExecutorError::ExecutionFailed(e.to_string()),
    }
}
```

---

## 7. Tracing

所有 `execute` 调用带完整 span：

```
tenant_id  team_id  role_id  squad_id  mission_id
```

`AgentInput` 新增字段（§8），`SquadEngine` 在调用前填入。

---

## 8. AgentInput 修改

```rust
pub struct AgentInput {
    pub task: String,
    pub context: TeamContext,
    pub model_override: Option<ModelConfig>,
    pub timeout: Option<Duration>,
    /// Squad identifier for tracing and persistence.
    pub squad_id: Option<String>,
    /// Mission identifier for tracing.
    pub mission_id: Option<String>,
}
```

---

## 9. AgentExecutor trait 修改

### 9.1 `flush()` 加入 trait

```rust
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    async fn resolve_role(&self, role_id: &str) -> Result<Role, AgentExecutorError>;

    async fn execute(&self, role_id: &str, input: AgentInput) -> Result<AgentOutput, AgentExecutorError>;

    async fn execute_stream(
        &self,
        role_id: &str,
        input: AgentInput,
    ) -> Result<BoxStream<'static, AgentOutputChunk>, AgentExecutorError>;

    /// Flush all cached session state to persistent storage.
    /// Default: no-op. Production implementations override.
    async fn flush(&self) -> Result<(), AgentExecutorError> {
        Ok(())
    }
}
```

### 9.2 `PandariaAgentExecutor::flush` 实现

```rust
async fn flush(&self) -> Result<(), AgentExecutorError> {
    let map = {
        self.sessions.lock()
            .expect("pandaria executor session map poisoned")
            .clone()
    };
    for (cache_key, actor_arc) in map {
        let mut actor = actor_arc.lock().await;
        actor.flush().await
            .map_err(|e| AgentExecutorError::ExecutionFailed(
                format!("flush {} failed: {}", cache_key, e)
            ))?;
    }
    Ok(())
}
```

---

## 10. SquadEngine 改动

`SquadEngine::execute_mission` 中构造 `AgentInput` 时填入新字段：

```rust
let input = AgentInput {
    task,
    context: squad.context.clone(),
    model_override: role.model_override.clone(),
    timeout: mission.timeout.map(std::time::Duration::from_secs),
    squad_id: Some(squad.id.clone()),
    mission_id: Some(mission.id.clone()),
};
```

Squad 结束时调用 `squad.executor.flush().await`：

```rust
// In SquadEngine::run, after completion or failure:
if let Err(e) = squad.executor.flush().await {
    tracing::warn!(error = %e, "squad executor flush failed");
}
```

---

## 11. 依赖

`tavern-comp` 已有依赖，无需新增：
- `agent-core` — `SessionBuilder`, `SessionActor`, `ToolConfig`, `HarnessConfig`, `AgentError`
- `ai-provider` — provider trait（间接依赖）
- `tokio` — `sync::Mutex`, `sync::Semaphore`
- `uuid` — session id
- `async-trait` — `AgentResolver` trait
- `futures-util` — stub stream
- `tracing` — instrument macro

---

## 12. 测试策略

### 12.1 单元测试（`#[cfg(test)] mod tests` in `pandaria_executor.rs`）

1. **`resolve_role` 正常**：Mock `AgentResolver` 返回 `AgentConfig`，验证 `Role` 字段。
2. **`resolve_role` 找不到**：返回 `RoleNotFound`。
3. **同 role+model 复用 session**：两次 `execute` 同一 role+model，验证第二次走 cache（通过 mock 统计 `resolve` 调用次数判断没重新 build）。
4. **不同 model 不同 session**：先调 `model_a`，再调 `model_b`，验证创建了两个 session。
5. **并发同一 key 串行化**：两个 task 同时 `execute` 同一 role+model，验证不 panic，都返回结果。
6. **`flush` 等待持久化**：mock store 记录 `save_session` 调用次数。
7. **semaphore 限制**：创建 `max_sessions=1` 的 executor，同时 spawn 3 个 session，验证只有 1 个存活。
8. **`map_agent_error`**：验证 `AgentError::ToolDenied` → `AgentExecutorError::ToolDenied` 等映射。

### 12.2 集成测试（`crates/tavern-comp/tests/pandaria_executor_integration.rs`）

- SquadEngine + PandariaAgentExecutor + MockProvider 端到端 DAG 模式。

---

## 13. 实现顺序

1. 修改 `AgentExecutorError`（新增 variant）
2. 修改 `AgentInput`（加 `squad_id`, `mission_id`）
3. 修改 `AgentExecutor` trait（加 `flush` 默认实现，加 `AgentResolver` trait）
4. 更新 mock executor 实现（适配 trait 改动）
5. 实现 `AgentResolver` for `TavernHero`
6. 实现 `PandariaAgentExecutor`（含 `build_tool_configs`, `build_role_prompt`, `map_agent_error`）
7. 修改 `SquadEngine::execute_mission`（填入 `squad_id`/`mission_id`，调用 `flush`）
8. 导出 `PandariaAgentExecutor`
9. 写单元测试
10. 写集成测试
11. 更新 `AGENTS.md` 和 `tavern-comp/README.md`

---

## 14. 已知限制

| 限制 | 状态 |
|---|---|
| `execute_stream` 为 collect-then-yield（非真流式） | ✅ 已升级为 per-chunk 流式（`SessionActor::complete_with_deltas` → LLM TextDelta 逐 chunk 转发） |
| 真流式输出（per-token streaming） | ✅ 已实现（`AgentLoopConfig::text_stream_tx` + `SessionActor::complete_with_deltas`） |
| `AgentOutput.usage` 填充 | ✅ 已实现（`SessionActor::last_usage()`） |
| Rust / Subprocess skill 工具化 | ✅ 已实现（通过 `with_tool_server()` 配置 base URL） |
| `execute` 内部不做 timeout | ✅ 由 SquadEngine 层 `Mission.timeout` 控制 |
| Session 从 cache 移除逻辑 | 后续 PR |
| `Drop` 时不自动 flush | ✅ SquadEngine::run 显式调用 flush |
| 真流式输出（per-token streaming） | 后续 PR（需 SessionActor 层流式支持） |

---

## 15. 相关文档

- `docs/superpowers/specs/2026-06-16-tavern-agent-team-design.md`
- `crates/tavern-comp/README.md`
- `AGENTS.md`
- `inline/pandaria-agent-executor-design-review.md`（review 原文）
