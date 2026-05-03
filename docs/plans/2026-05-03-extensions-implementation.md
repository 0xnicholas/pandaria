# extensions 开发计划

**Date:** 2026-05-03
**Status:** Draft
**Priority:** P0（核心基础设施）
**Reference:** `docs/specs/2026-05-02-extensions.md`, `AGENTS.md` (ADR-002, ADR-003)
**Current Code:** `crates/extensions/src/` (6 个文件，MVP 实现)

---

## 概述

将 extensions crate 从当前 MVP（7 个 hook 方法、3 个 ActorMessage 变体、泛型 EventBus）升级到完整 spec 定义（14 个 hook 方法、execute_tool、8 个 ActorMessage 变体、专门 ObservationEvent、ExtensionManager、ExtensionTool、3 个内置扩展、完整测试套件）。

**前置硬阻塞条件:**
- **agent-core Phase 0 必须完成** (`docs/plans/2026-05-03-agent-core-implementation.md`)
- 需要 agent-core 交付 8 个新 Ctx 类型 + 5 个新 Mutation 类型（见下方表格）
- **在 agent-core Phase 0 完成前，extensions 无法编译通过**

**开发顺序（联合视图）:**
```
Week 1: 等待 agent-core Phase 0 完成（阻塞期）
        └─ 可提前准备：T1.1 ToolCallMutation 本地定义（临时，后续迁移）
        
Week 1-2: Phase 1-4 (P0)  [与 agent-core Phase 1-4 并行]
Week 2-3: Phase 5 (P1)     [与 agent-core Phase 5-7 并行]
Week 3-4: Phase 6-7 (P1)   [与 agent-core Phase 8-11 并行]
```

### 当前基线

```
crates/extensions/src/
  lib.rs                  # re-exports: Extension, EventBus, HookRouter
  host/
    mod.rs                # 模块声明
    extension.rs          # Extension trait（7 个 hook 方法）
    extension_actor.rs    # ExtensionActor（3 个 ActorMessage 变体，无 panic 隔离）
    hook_router.rs        # HookRouter（实现 6 个 HookDispatcher 方法）
    event_bus.rs          # 泛型 EventBus<T>，ObsEvent 仅 3 个变体
  builtins/
    mod.rs                # 占位
```

### 目标状态

```
crates/extensions/src/
  lib.rs                  # re-exports
  host/
    mod.rs
    extension.rs          # Extension trait（14 hooks + execute_tool + ToolCallMutation）
    extension_actor.rs    # ExtensionActor（8 个 ExtensionCommand + Shutdown，panic 隔离）
    hook_router.rs        # HookRouter（14 个 HookDispatcher 方法，input mutation chain）
    event_bus.rs          # EventBus（专门 ObservationEvent，7 个变体）
    manager.rs            # ExtensionManager（spawn_all, collect_tools, collect_agent_tools）
    extension_tool.rs     # ExtensionTool（AgentTool trait 包装器）
  builtins/
    mod.rs
    audit.rs              # AuditExtension
    rate_limit.rs         # RateLimitExtension
    tool_guard.rs         # ToolGuardExtension
  tests/
    hook_router_tests.rs
    extension_actor_tests.rs
    event_bus_tests.rs
    builtin_audit_tests.rs
    builtin_rate_limit_tests.rs
    builtin_tool_guard_tests.rs
```

### 开发原则

- **Spec 驱动**：以 `docs/specs/2026-05-02-extensions.md` 为目标
- **测试先行**：每个模块先写测试，再写实现
- **增量可编译**：每步完成后 `cargo build -p extensions` 通过
- **agent-core 依赖管理**：agent-core 也在并行开发。本计划明确标注每个任务对 agent-core 新类型的依赖，必要时使用占位类型或条件编译保持编译通过

---

## 前置硬依赖：agent-core Phase 0 交付清单

> **⚠️ 阻塞声明**: extensions 编译**绝对依赖** agent-core Phase 0 完成。在以下类型可用前，extensions Phase 1-4 无法启动。

### 必须由 agent-core Phase 0 交付的类型

| 类型 | 位置 | 用途 | 阻塞的任务 |
|---|---|---|---|
| `CompactCtx` | `agent_core::context` | `on_before_compact` hook 参数 | T1.2, T4.2 |
| `BeforeAgentStartCtx` | `agent_core::context` | `on_before_agent_start` hook 参数 | T1.2, T4.3 |
| `ProviderRequestCtx` | `agent_core::context` | `on_before_provider_request` hook 参数 | T1.2, T4.3 |
| `ProviderResponseCtx` | `agent_core::context` | `on_after_provider_response` hook 参数 | T1.2, T4.3 |
| `ToolExecutionStartCtx` | `agent_core::context` | `on_tool_execution_start` 观测参数 | T1.2, T4.4 |
| `ToolExecutionUpdateCtx` | `agent_core::context` | `on_tool_execution_update` 观测参数 | T1.2, T4.4 |
| `ToolExecutionEndCtx` | `agent_core::context` | `on_tool_execution_end` 观测参数 | T1.2, T4.4 |
| `CompactEndCtx` | `agent_core::context` | `on_compact_end` 观测参数 | T1.2, T4.4 |
| `CompactDecision` | `agent_core::mutations` | `on_before_compact` 返回值 | T1.2, T4.2 |
| `BeforeAgentStartMutation` | `agent_core::mutations` | `on_before_agent_start` 返回值 | T1.2, T4.3 |
| `ProviderRequestMutation` | `agent_core::mutations` | `on_before_provider_request` 返回值 | T1.2, T4.3 |
| `ProviderResponseMutation` | `agent_core::mutations` | `on_after_provider_response` 返回值 | T1.2, T4.3 |
| `ToolCallMutation` | `agent_core::mutations` | `on_tool_call` 返回值 | T1.1, T1.2, T4.1 |
| `HookDispatcher` 扩展 | `agent_core::hook_dispatcher` | 从 6 个方法扩展到 14 个 | T4.x |

### 执行策略

**方案 A（推荐）**: 等待 agent-core Phase 0 完成后启动 extensions Phase 1-4
- 优点：零技术债，类型定义单一来源
- 缺点：extensions 第一周可能有空窗期

**方案 B（备选）**: extensions 先本地定义占位类型，agent-core 完成后再迁移
- 适用场景：如果 agent-core Phase 0 延迟交付
- 风险：需要两次修改（定义 → 迁移 → 删除），增加合并冲突概率

**当前决策**: 采用方案 A。extensions 开发者在 agent-core Phase 0 完成前可提前阅读 spec、准备测试用例。

---

## Phase 1: Extension trait 升级 (P0 — 阻塞解除后立即启动)

**前置条件**: agent-core Phase 0 完成（8 Ctx + 5 Mutation 类型可用）

### T1.1 定义 ToolCallMutation (P0)

**文件**: `crates/extensions/src/host/extension.rs`
**依赖**: 无（本地定义）
**Spec**: §3

```rust
#[derive(Debug, Clone, Default)]
pub struct ToolCallMutation {
    pub input: Option<serde_json::Value>,
}
```

### T1.2 扩展 Extension trait (P0)

**文件**: `crates/extensions/src/host/extension.rs`
**依赖**: agent-core 新类型（CompactCtx, BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx, ToolExecutionStartCtx, ToolExecutionUpdateCtx, ToolExecutionEndCtx, CompactEndCtx, CompactDecision, BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation）
**Spec**: §3

变更：
- `on_tool_call` 返回类型从 `HookDecision` 改为 `(HookDecision, ToolCallMutation)`
- 新增 hooks:
  - `on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision`
  - `on_before_agent_start(&self, _ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation`
  - `on_before_provider_request(&self, _ctx: &ProviderRequestCtx) -> ProviderRequestMutation`
  - `on_after_provider_response(&self, _ctx: &ProviderResponseCtx) -> ProviderResponseMutation`
  - `execute_tool(&self, _tool_call_id: &str, _params: serde_json::Value) -> Result<AgentToolResult, AgentError>`
  - `on_tool_execution_start(&self, _ctx: &ToolExecutionStartCtx)`
  - `on_tool_execution_update(&self, _ctx: &ToolExecutionUpdateCtx)`
  - `on_tool_execution_end(&self, _ctx: &ToolExecutionEndCtx)`
  - `on_compact_end(&self, _ctx: &CompactEndCtx)`

**验证**: `cargo build -p extensions` 通过

---

## Phase 2: ExtensionActor 重构 (P0)

### T2.1 重命名并扩展命令枚举 (P0)

**文件**: `crates/extensions/src/host/extension_actor.rs`
**依赖**: T1.2 完成的 Extension trait 新签名
**Spec**: §4.1

将 `ActorMessage` 重命名为 `ExtensionCommand`，新增变体：
- `OnBeforeCompact { ctx, reply }`
- `OnBeforeAgentStart { ctx, reply }`
- `OnBeforeProviderRequest { ctx, reply }`
- `OnAfterProviderResponse { ctx, reply }`
- `OnExecuteTool { tool_call_id, params, reply }`
- `Shutdown`

### T2.2 重构 Actor 循环 (P0)

**文件**: `crates/extensions/src/host/extension_actor.rs`
**依赖**: T2.1
**Spec**: §4.3

变更：
- 在 `run_actor` 中引入 `tokio::select!` 同时监听 mpsc mailbox 和 broadcast EventBus
- 观测事件处理移到 actor 循环内（替代当前的 `spawn_listener` 方案）
- 阻断/链式 hook 使用 `tokio::spawn` 包装实现 panic 隔离
- `OnExecuteTool` 通过 `tokio::spawn` 异步执行，无超时

### T2.3 扩展 ExtensionHandle (P0)

**文件**: `crates/extensions/src/host/extension_actor.rs`
**依赖**: T2.1
**Spec**: §4.5

新增方法：
- `ask<T>()` 泛型辅助方法（带超时）
- `execute_tool()`（无超时，专门用于工具执行）
- `shutdown()`（发送 Shutdown 命令）

定义 `AskError` 枚举：
```rust
enum AskError {
    Timeout,
    ActorGone,
}
```

### T2.4 添加 HookByName 辅助 trait (P0)

**文件**: `crates/extensions/src/host/extension_actor.rs`
**依赖**: T1.2
**Spec**: §4.4

定义 `HookByName<Ctx>` trait，为每种 hook 上下文类型实现 `dispatch()` 方法，避免 actor 循环中的重复 match。

**验证**: `cargo build -p extensions` 通过，`cargo test -p extensions -- extension_actor` 通过（现有测试更新 + 新增测试）

---

## Phase 3: EventBus 升级 (P0)

### T3.1 定义 ObservationEvent (P0)

**文件**: `crates/extensions/src/host/event_bus.rs`
**依赖**: agent-core 新类型（ToolExecutionStartCtx 等）
**Spec**: §5

将 `EventBus<T>` 从泛型改为专门类型：
```rust
#[derive(Debug, Clone)]
pub enum ObservationEvent {
    TurnEnd(TurnEndCtx),
    AgentEnd(AgentEndCtx),
    SessionStart(SessionCtx),
    ToolExecutionStart(ToolExecutionStartCtx),
    ToolExecutionUpdate(ToolExecutionUpdateCtx),
    ToolExecutionEnd(ToolExecutionEndCtx),
    CompactEnd(CompactEndCtx),
}

pub struct EventBus {
    tx: broadcast::Sender<ObservationEvent>,
}
```

### T3.2 更新 EventBus API (P0)

**文件**: `crates/extensions/src/host/event_bus.rs`
**依赖**: T3.1
**Spec**: §5

- `emit()` 添加 `tracing::warn!` 当无订阅者时
- `subscribe()` 返回 `broadcast::Receiver<ObservationEvent>`
- 添加 `variant_name()` 辅助方法用于日志

### T3.3 更新 ObsEvent 使用者 (P0)

**文件**: `crates/extensions/src/host/hook_router.rs`, `crates/extensions/src/host/extension_actor.rs`
**依赖**: T3.1
**Spec**: §5

将 `ObsEvent` 替换为 `ObservationEvent`，更新所有 match 分支。

**验证**: `cargo build -p extensions` 通过

---

## Phase 4: HookRouter 升级 (P0)

### T4.1 更新 on_tool_call（支持 input mutation） (P0)

**文件**: `crates/extensions/src/host/hook_router.rs`
**依赖**: T1.1（ToolCallMutation）, T2.3（ExtensionHandle::ask）
**Spec**: §6.1

实现 input mutation chain：
- 每个 handler 看到的 `ctx.input` 是前一个 handler 修改后的值
- 即使 Block，前面 handler 的 mutation 仍然保留
- 返回 `(HookDecision, ToolCallMutation)`

### T4.2 实现新的阻断型 hook：on_before_compact (P0)

**文件**: `crates/extensions/src/host/hook_router.rs`
**依赖**: agent-core `CompactDecision`, `CompactCtx`
**Spec**: §6.5

实现 first-block-wins 语义，支持 `Block { reason }` 和 `Replace { result }`。

### T4.3 实现新的链式 hooks (P0)

**文件**: `crates/extensions/src/host/hook_router.rs`
**依赖**: agent-core 新 mutation 类型
**Spec**: §6.6, §6.7, §6.8

- `on_before_agent_start`：链式合并，支持 `system_prompt` 和 `messages` 字段
- `on_before_provider_request`：链式合并，支持 `system_prompt`, `messages`, `tools`, `options`
- `on_after_provider_response`：链式合并，支持 `content`, `stop_reason`

每个链式 hook 需要：
1. 维护 `current_ctx`（应用 mutation 后传给下一个 handler）
2. 维护 `accumulated` mutation（最终返回）
3. 超时处理：跳过该 handler，继续后续

### T4.4 实现新的观测型 hooks (P0)

**文件**: `crates/extensions/src/host/hook_router.rs`
**依赖**: T3.1（ObservationEvent）
**Spec**: §6.4

- `on_tool_execution_start`
- `on_tool_execution_update`
- `on_tool_execution_end`
- `on_compact_end`

所有观测型 hook 都通过 `event_bus.emit()` 广播，立即返回。

### T4.5 添加 mutation 合并辅助函数 (P0)

**文件**: `crates/extensions/src/host/hook_router.rs`
**依赖**: T4.3
**Spec**: §6.2

定义：
- `apply_tool_result_mutation(ctx, mutation)`
- `merge_tool_result_mutation(acc, mutation)`
- `apply_provider_request_mutation(ctx, mutation)`
- `merge_provider_request_mutation(acc, mutation)`

**验证**: `cargo build -p extensions` 通过，`cargo test -p extensions -- hook_router` 通过

---

## Phase 5: ExtensionManager + ExtensionTool (P1)

### T5.1 实现 ExtensionManager (P1)

**新建文件**: `crates/extensions/src/host/manager.rs`
**依赖**: T2.3（ExtensionHandle）, T4.x（HookRouter）
**Spec**: §7

功能：
- `new(extensions: Vec<Arc<dyn Extension>>) -> Self`
- `collect_tools() -> Vec<ToolDef>`（first-registration-wins 去重）
- `spawn_all(tenant_id, session_id) -> (HookRouter, Vec<ExtensionHandle>, Vec<JoinHandle<()>>)`
- `collect_agent_tools(handles, extensions) -> Vec<AgentToolRef>`（构造 ExtensionTool）

### T5.2 实现 ExtensionTool (P1)

**新建文件**: `crates/extensions/src/host/extension_tool.rs`
**依赖**: T2.3（ExtensionHandle::execute_tool）
**Spec**: §8

实现 `AgentTool` trait：
- 包装 Extension 的 `tools()` 定义和 `execute_tool()` 执行
- `execute()` 委托给 `ExtensionHandle::execute_tool()`
- v0.1 不支持进度回调（`on_progress` 忽略）

### T5.3 更新模块导出 (P1)

**文件**: `crates/extensions/src/host/mod.rs`, `crates/extensions/src/lib.rs`
**依赖**: T5.1, T5.2

添加 `pub mod manager;` 和 `pub mod extension_tool;`。

**验证**: `cargo build -p extensions` 通过

---

## Phase 6: 内置扩展 (P1)

### T6.1 AuditExtension (P1)

**新建文件**: `crates/extensions/src/builtins/audit.rs`
**依赖**: T1.2（完整 Extension trait）
**Spec**: §9.1

- `on_tool_call`：记录 tool_call_start 到 tracing
- `on_tool_result`：记录 tool_call_end 到 tracing
- `on_turn_end`：记录 turn_end 到 tracing
- 永不阻断，永不修改

### T6.2 RateLimitExtension (P1)

**新建文件**: `crates/extensions/src/builtins/rate_limit.rs`
**依赖**: T1.2
**Spec**: §9.2

- 滑动窗口计数（60 秒）
- `std::sync::Mutex<Vec<Instant>>` 存储调用时间
- 超限返回 `Block { reason }`

### T6.3 ToolGuardExtension (P1)

**新建文件**: `crates/extensions/src/builtins/tool_guard.rs`
**依赖**: T1.2
**Spec**: §9.3

- `allowed_tools`：白名单（非空时只允许列表内工具）
- `denied_tools`：黑名单（优先级高于白名单）

### T6.4 更新 builtins/mod.rs (P1)

**文件**: `crates/extensions/src/builtins/mod.rs`
**依赖**: T6.1, T6.2, T6.3

导出三个内置扩展。

**验证**: `cargo build -p extensions` 通过

---

## Phase 7: 测试 (P1)

### T7.1 HookRouter 测试 (P1)

**新建文件**: `crates/extensions/tests/hook_router_tests.rs`
**依赖**: T4.x（完整 HookRouter）
**Spec**: §11.1

覆盖 20 个测试用例：
- 阻断型 first-block-wins（3 个）
- input mutation chain（4 个）
- chain merge（4 个）
- 观测型广播（2 个）
- before_agent_start chain（2 个）
- before_compact（3 个）
- tool_execution / compact_end 广播（2 个）

### T7.2 ExtensionActor 测试 (P1)

**新建文件**: `crates/extensions/tests/extension_actor_tests.rs`
**依赖**: T2.x（完整 ExtensionActor）
**Spec**: §11.2

覆盖 8 个测试用例：
- 启动/关闭（1 个）
- on_tool_call 回复（1 个）
- panic 隔离（2 个）
- oneshot 超时（1 个）
- EventBus 接收（1 个）
- 观测超时（1 个）
- Shutdown 后 ActorGone（1 个）

### T7.3 EventBus 测试 (P1)

**新建文件**: `crates/extensions/tests/event_bus_tests.rs`
**依赖**: T3.x（完整 EventBus）
**Spec**: §5

覆盖：
- emit/receive（基本功能）
- 多订阅者接收
- Lagged 处理
- 无订阅者时 warning 日志

### T7.4 内置扩展测试 (P1)

**新建文件**:
- `crates/extensions/tests/builtin_audit_tests.rs`
- `crates/extensions/tests/builtin_rate_limit_tests.rs`
- `crates/extensions/tests/builtin_tool_guard_tests.rs`

**依赖**: T6.x（内置扩展）
**Spec**: §11.3

覆盖 10 个测试用例（audit 3 个，rate-limit 3 个，tool-guard 4 个）。

**验证**: `cargo test -p extensions` 全部通过

---

## 任务依赖图

```
T1.1 ──→ T1.2 ──→ T2.1 ──→ T2.2 ──→ T2.3 ──→ T2.4
                       │
                       ▼
                   T3.1 ──→ T3.2 ──→ T3.3
                       │
                       ▼
                   T4.1 ──→ T4.2 ──→ T4.3 ──→ T4.4 ──→ T4.5
                       │
                       ▼
                   T5.1 ──→ T5.2 ──→ T5.3
                       │
                       ▼
                   T6.1 ──→ T6.2 ──→ T6.3 ──→ T6.4
                       │
                       ▼
                   T7.1 ──→ T7.2 ──→ T7.3 ──→ T7.4
```

**并行机会**：
- T3.x（EventBus）和 T2.x（ExtensionActor）可部分并行，只要 ObservationEvent 定义确定
- T6.x（内置扩展）之间完全独立，可并行开发
- T7.x（测试）之间完全独立，可并行编写

---

## 时间估算与优先级

| Phase | 内容 | 预估时间 | 优先级 | 前置条件 |
|---|---|---|---|---|
| Phase 1 | Extension trait 升级 | 30 min | **P0** | agent-core Phase 0 |
| Phase 2 | ExtensionActor 重构 | 2h | **P0** | Phase 1 |
| Phase 3 | EventBus 升级 | 30 min | **P0** | Phase 1 |
| Phase 4 | HookRouter 升级 | 2h | **P0** | Phase 2-3 |
| Phase 5 | ExtensionManager + ExtensionTool | 1.5h | **P1** | Phase 4 |
| Phase 6 | 内置扩展（3 个） | 1.5h | **P1** | Phase 5 |
| Phase 7 | 测试套件 | 3h | **P1** | Phase 6 |
| **P0 小计** | | **~5h** | | |
| **P1 小计** | | **~6h** | | |
| **总计** | | **~11h** | | |

**P0 核心路径**: Phase 1 → Phase 2 → Phase 3 → Phase 4（约 5h）
**P0 完成后 agent-core 即解除阻塞**，extensions 可与 agent-core 后续 Phase 并行开发。

---

## 验证检查点

每 task 完成后执行：

```bash
cargo build -p extensions          # 编译通过
cargo test -p extensions -- <module>  # 对应模块测试通过
```

**Phase 1-4 完成时**（核心基础设施）：
```bash
cargo build -p extensions          # 编译通过
cargo test -p extensions           # 现有测试通过
```

**全部完成时**：
```bash
cargo test -p extensions           # 全部测试通过
cargo clippy -p extensions         # 无 lint 警告
```

---

## agent-core 依赖跟踪

| extensions 任务 | 所需 agent-core 类型 | 状态 | 备注 |
|---|---|---|---|
| T1.2 | CompactCtx, BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx, ToolExecution*Ctx, CompactEndCtx, CompactDecision, BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation | 🔲 待提供 | 如未提供，使用占位类型 |
| T4.2 | CompactDecision, CompactCtx | 🔲 待提供 | 同上 |
| T4.3 | BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation | 🔲 待提供 | 同上 |
| T4.4 | ToolExecution*Ctx, CompactEndCtx | 🔲 待提供 | 同上 |
| T5.1 | AgentToolRef, AgentTool | 🟡 部分存在 | 确认 agent-core 当前已导出 |

**建议**：在计划开始时与 agent-core 负责人确认这些类型的交付时间线。如果 agent-core 延迟，extensions 可先使用本地占位类型保持开发节奏，后续迁移。

---

## Breaking Change 管理

1. **Extension::on_tool_call 返回类型变更**：从 `HookDecision` 改为 `(HookDecision, ToolCallMutation)`。HookRouter 和 ExtensionActor 需同步更新。
   - **解决**: `ToolCallMutation` 由 agent-core Phase 0 统一定义，extensions 直接使用 `use agent_core::mutations::ToolCallMutation`
2. **EventBus 从泛型改为专门类型**：影响 `lib.rs` 导出和外部使用者（如 HookRouter）。
3. **新增文件**：manager.rs, extension_tool.rs, builtins/*.rs 需要更新 `mod.rs`。

处理策略：每个 task 独立提交，保持 git 历史清晰。P0 任务优先合并，P1 任务可分批提交。

---

## 新增文件汇总

| 文件 | Phase | Spec 章节 |
|---|---|---|
| `src/host/manager.rs` | P5 | §7 |
| `src/host/extension_tool.rs` | P5 | §8 |
| `src/builtins/audit.rs` | P6 | §9.1 |
| `src/builtins/rate_limit.rs` | P6 | §9.2 |
| `src/builtins/tool_guard.rs` | P6 | §9.3 |
| `tests/hook_router_tests.rs` | P7 | §11.1 |
| `tests/extension_actor_tests.rs` | P7 | §11.2 |
| `tests/event_bus_tests.rs` | P7 | §5 |
| `tests/builtin_audit_tests.rs` | P7 | §11.3 |
| `tests/builtin_rate_limit_tests.rs` | P7 | §11.3 |
| `tests/builtin_tool_guard_tests.rs` | P7 | §11.3 |
