# 第三章：Hook 系统

> **目标读者**：想写自定义 hook 策略或排查 hook 行为的开发者。  
> **前提**：已读第二章（Agent Loop），了解 agent 执行流程。

---

## 3.1 设计哲学

Pandaria 的 hook 系统是**直接函数调用**（ADR-003），而非 Actor mailbox。原因是：

| Actor 模型（已废弃） | 直接函数调用（当前） |
|---|---|
| 每个 hook 通过 mpsc channel 发送消息 | 同步函数调用 |
| 500ms/100ms 超时 | 无内置超时（各策略自行管理） |
| Extension panic 被 Actor 隔离 | Panic 直接传播到 AgentLoop/ToolExecutor |
| 运行时开销：channel + oneshot + clone | 零开销 |
| 支持第三方 WASM/RPC 扩展 | 不支持（未来重新设计） |

**权衡**：失去了 panic 隔离和动态扩展能力，换来了极简的执行路径和易于调试的调用栈。

---

## 3.2 三种 Hook 类型

### 阻断型（First-Block-Wins）

在工具调用或 compaction 执行**之前**拦截，第一个返回 `Block` 的策略胜出。

```
on_tool_call()
  │
  ├─→ PathGuard::on_tool_call()     → Allow
  ├─→ ToolGuard::on_tool_call()     → Block("tool not in allow list")
  └─→ ⚡ 返回 Block，后续策略不再执行
```

**阻断型 hook 列表**：
- `on_tool_call` — 工具执行前拦截
- `on_before_compact` — 压缩执行前拦截

---

### 链式合并（Chain）

每个 hook 接收上一个 hook 的输出，可以修改数据后传给下一个。

```
on_tool_result(tool_result)
  │
  ├─→ ContentFilter::on_tool_result()    → tool_result' (PII 脱敏)
  ├─→ Audit::on_tool_result()            → tool_result' (添加审计日志，不变更内容)
  └─→ 返回最终 tool_result''
```

**链式 hook 列表**：
- `on_tool_result` — 工具执行结果处理
- `on_context` — 上下文注入（追加/替换 prompt 内容）
- `on_before_agent_start` — Agent 启动前的 prompt 突变
- `on_before_provider_request` — LLM 请求前的最后修改
- `on_after_provider_response` — LLM 响应后的处理

---

### 观测型（Observe）

纯副作用——不返回值，不影响执行流。用于日志、指标、事件投递。

```
on_turn_end()
  │
  ├─→ Audit::on_turn_end()         → 记录日志
  ├─→ TokenBudget::on_turn_end()   → 更新配额计数
  ├─→ MemoryHook::on_turn_end()    → 保存对话到 Emerald（fire-and-forget）
  └─→ (无返回值)
```

**观测型 hook 列表**：
- `on_turn_end` — 每轮对话结束时
- `on_agent_end` — Agent session 终止时
- `on_session_start` — Session 创建时
- `on_tool_call` — 工具被调用时（审计维度，与阻断型共用入口）
- `on_compact_end` — 压缩完成时

---

## 3.3 DefaultHookDispatcher 内置策略

```rust
// agent-core/src/hook/default_dispatcher.rs

pub struct DefaultHookDispatcher {
    pub space: AgentSpace,
    pub denied_tools: Vec<String>,      // ToolGuard: 禁止的工具名单
    pub allowed_tools: Vec<String>,     // ToolGuard: 允许的工具名单
    pub path_guard_fields: HashMap<String, Vec<String>>,  // PathGuard: 字段映射
    pub path_guard_scan_unknown: bool,  // PathGuard: 是否扫描未知工具
    pub max_turns: Option<u32>,         // TokenBudget: 最大 turn 数
    pub pii_patterns: Vec<Regex>,       // ContentFilter: PII 正则
    pub memory_store: Option<Arc<dyn MemoryStore>>,  // MemoryHook: 记忆后端
}
```

### 3.3.1 Audit

**类型**：观测型  
**触发点**：`on_tool_call`、`on_tool_result`、`on_turn_end`、`on_agent_end`

在每个关键节点打 `tracing` 日志，携带 `tenant_id` 和 `session_id`。不修改任何数据。

### 3.3.2 PathGuard

**类型**：阻断型（`on_tool_call`）  
**作用**：防止 agent 访问 workspace 以外的文件路径。

```rust
// 检查逻辑（伪代码）
fn on_tool_call(&self, ctx: &ToolCallCtx) -> HookDecision {
    let paths = extract_paths(ctx);  // 从工具参数中提取路径
    let workspace = self.space.workspace_for(&ctx.tenant_id);

    for path in &paths {
        let canonical = std::fs::canonicalize(path)?;
        if !canonical.starts_with(&workspace) {
            return HookDecision::Block("path outside workspace");
        }
    }
    HookDecision::Allow
}
```

**配置**：`path_guard_fields` 指定每个工具的哪些参数包含路径。`path_guard_scan_unknown = true` 时，对未知工具自动扫描所有字符串参数。

### 3.3.3 ToolGuard

**类型**：阻断型（`on_tool_call`）  
**作用**：基于白名单/黑名单控制哪些工具可用。

```rust
fn on_tool_call(&self, ctx: &ToolCallCtx) -> HookDecision {
    if !self.allowed_tools.is_empty()
        && !self.allowed_tools.contains(&ctx.tool_name) {
        return HookDecision::Block("tool not in allowed list");
    }
    if self.denied_tools.contains(&ctx.tool_name) {
        return HookDecision::Block("tool in denied list");
    }
    HookDecision::Allow
}
```

### 3.3.4 TokenBudget

**类型**：观测型（`on_turn_end`）  
**作用**：追踪每个 session 的 turn 数，超过 `max_turns` 时打 warning 日志。**不阻断执行**（非阻塞型——仅日志）。

### 3.3.5 ContentFilter

**类型**：链式（`on_tool_result`）  
**作用**：对工具输入和输出做 PII 脱敏（正则匹配 + 替换）。

### 3.3.6 MemoryHook

**类型**：观测型（`on_turn_end`、`on_before_agent_start`）  
**作用**：通过 `MemoryStore` trait 接驳外部记忆系统（默认是 `EmeraldMemoryStore`）。

- `on_before_agent_start`：调用 `MemoryStore::recall()` 拉取用户画像/记忆，注入 prompt
- `on_turn_end`：调用 `MemoryStore::remember()` 保存当前轮对话

---

## 3.4 HookDispatcher trait

所有 hook dispatcher 必须实现此 trait：

```rust
pub trait HookDispatcher: Send + Sync {
    fn on_tool_call(&self, ctx: &ToolCallCtx) -> HookDecision;
    fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation;
    fn on_turn_end(&self, ctx: &TurnEndCtx);
    fn on_agent_end(&self, ctx: &AgentEndCtx);
    fn on_before_compact(&self, ctx: &CompactCtx) -> CompactDecision;
    fn on_before_agent_start(&self, ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation;
    fn on_before_provider_request(&self, ctx: &ProviderRequestCtx) -> ProviderRequestMutation;
    fn on_after_provider_response(&self, ctx: &ProviderResponseCtx) -> ProviderResponseMutation;
}
```

---

## 3.5 编写自定义 HookDispatcher

有两种方式：

### 方式 A：替换 DefaultHookDispatcher

实现 `HookDispatcher` trait，从零开始：

```rust
struct MyDispatcher;

impl HookDispatcher for MyDispatcher {
    fn on_tool_call(&self, ctx: &ToolCallCtx) -> HookDecision {
        // 自定义逻辑
        HookDecision::Allow
    }
    // ... 实现其他方法
}
```

然后在 `TenantManagerImpl::create_session()` 中注入。

### 方式 B：包装 DefaultHookDispatcher（推荐）

保留内置策略，叠加自定义：

```rust
struct MyDispatcher {
    inner: DefaultHookDispatcher,
}

impl HookDispatcher for MyDispatcher {
    fn on_tool_call(&self, ctx: &ToolCallCtx) -> HookDecision {
        // 先跑内置策略
        let decision = self.inner.on_tool_call(ctx);
        if let HookDecision::Block(_) = decision {
            return decision;  // 内置策略已拦截
        }
        // 叠加自定义逻辑...
        HookDecision::Allow
    }

    // 其他方法 delegate 给 inner
    fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
        self.inner.on_tool_result(ctx)
    }
    // ...
}
```

---

## 3.6 注意事项

1. **Hook 是同步调用**：避免在 hook 中执行长时间阻塞操作。如需异步 I/O（如 HTTP 请求），内部 spawn tokio task（见 [Emerald 异步化方案](../specs/2026-05-28-ecosystem-integration-deepening.md#35-链路-eemerald-memory-异步化)）
2. **Panic 直接暴露**：没有 Actor 隔离，hook 内部的 panic 会传播到 AgentLoop
3. **阻断型 hook 的短路语义**：第一个 Block 胜出，后续 hook 不执行——策略顺序很重要
4. **链式 hook 的合并语义**：每个 hook 的输出是下一个的输入——确保不丢失前一个 hook 的修改

---

## 3.7 下一步

- 理解生态全景 → [第四章：生态项目概览](./04-ecosystem.md)
- 理解记忆集成细节 → [第五章：集成指南](./05-integration.md)
