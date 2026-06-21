# SessionActor 拆分重构 — 设计文档

> 2026-06-21 | 状态：设计中 | 范围：`crates/agent-core/src/harness/session/`

## 1. 动机

`crates/agent-core/src/harness/session.rs` 当前 **2625 行**，`SessionActor` 直接持有 29 个字段，涵盖：

- 身份 / 配置（`tenant_id`、`model`、`prompt_builder`、`skills` 等）
- 消息历史 + 持久化（`entries`、`last_saved_entry_count`、`last_save: JoinHandle`、`store`、`needs_restore`）
- 事件系统（`event_listeners`、`event_tx`、`event_processor_handle`）
- steer / follow-up 队列（`steer_queue`、`follow_up_queue`）
- 状态机 / 恢复（`state: AtomicU8`、`error_reason`、`recovery: RecoveryStateMachine`）
- 取消控制（`abort_token`）
- 编排逻辑（`prompt`、`run_with_messages`、`run_goal_sync`、`spawn_background_loop`）
- Goal 策略（`build_initial_goal_prompt`、`evaluate_criteria` 等）

痛点：

- **`run_with_messages` 单方法 ~250 行**，串起状态机、持久化、事件、compaction、recovery，新功能改一处追多处。
- 字段分组语义模糊，但代码物理上耦合——所有字段都是 `pub(crate)` 直接 `&mut self.xxx` 访问。
- 4 个紧密耦合的字段（`state`、`error_reason`、`recovery`、`abort_token`）在 100+ 个方法间以不同顺序读写。
- `#[allow(dead_code)] session_started_at` 暴露字段膨胀到无法精简单字段（本设计**明确移除**该字段：`grep -r session_started_at crates/` 在 agent-core 内部无任何 reader，仅 `new()` 初始化）。
- 模块演进受阻：未来给 history / event_hub / state 加独立 trait 或 mock 困难。

目标：拆成 3 个子系统 + 1 个瘦 orchestrator，**2625 行 → 4 个文件，每个 ≤ 800 行**。

## 2. 目标与非目标

### 目标

- 把 `SessionActor` 的 29 个字段重组为顶层字段 + 3 个子系统子结构
- 公开 API 100% 向后兼容（`SessionConfig`、`BuiltSession`、`SessionState`、`dummy_for_test`、`SessionBuilder` 调用方式全部不变）
- 现有 **22 个** SessionActor 单元测试**保持原样通过**（其中 2 处访问私有字段的测试通过新增的 `#[cfg(any(test, feature = "testing"))] pub(crate)` accessor 兼容，详见 §10）
- 新增 14 个子系统的独立单元测试（mock store、test listener）
- 不修改 `RecoveryStateMachine`、`AgentLoop`、`AgentLoopConfig`、其他 crate

### 非目标（本次明确不做）

- ❌ 重构 `AgentLoopConfig` 的 `#[doc(hidden)]` 字段（独立 issue）
- ❌ 提取 `HistoryStore` / `EventHub` trait 为 dyn-compatible（过早抽象）
- ❌ 重命名 `last_usage` 或重新设计 usage 跟踪
- ❌ 改 `RecoveryStateMachine` 内部状态机
- ❌ 修改 `HookDispatcher` trait
- ❌ 修改 `SessionBuilder` 或 `HarnessConfig`

## 3. 设计决策

| 决策 | 选择 | 理由 |
|---|---|---|
| 子系统粒度 | 3 个（History / EventHub / StateMachine） | ROI 最高，认知负担最低 |
| 所有权模式 | 值类型子结构（不是 Arc 共享） | 字段直接访问，无间接调用；session 独占，无跨 session 共享需求 |
| abort_token 归属 | `SessionStateMachine`（与 state/recovery 同生命周期） | 三者生命周期一致；`SessionActor::abort_token()` 委托返回 |
| 模块布局 | `session/` 目录替代 `session.rs` 单文件 | Rust 允许同名 mod.rs / 目录共存 |
| API 公开粒度 | 3 个子系统全 `pub` + SessionActor getter | 下游 crate（tavern-comp）可 mock；测试方便 |
| 内部跨子系统访问 | 通过 `pub(crate)` getter 暴露 `event_tx` / `steer_queue` / `follow_up_queue` 的克隆 | SessionActor 构造 `AgentLoopConfig` 时需要这些原始 Arc，避免暴露所有字段 |
| 测试字段访问 | `#[cfg(any(test, feature = "testing"))] pub(crate)` accessor（`entries_mut`、`abort_token_ref`） | 与现有 `dummy_for_test()` 和 `test_utils` 模块的 cfg 一致；不修改测试代码，仅新增 2 个 test-only 访问器 |
| `QueuedEvent` 可见性 | `pub(crate) struct QueuedEvent` 在 `event_hub.rs` | SessionActor 的 `event_sink` 闭包需要引用 |
| 迁移方式 | 渐进式 6 阶段，每次编译+测试验证 | 不破坏中间状态，回退容易 |

## 4. 模块结构

```
crates/agent-core/src/harness/
  session/
    mod.rs           # SessionActor (瘦 orchestrator) + SessionConfig + re-exports
    history.rs       # SessionHistory       (~280 行)
    event_hub.rs     # SessionEventHub     (~220 行)
    state.rs         # SessionStateMachine (~180 行)
  mod.rs             # pub mod session;  (改为目录)
```

`crates/agent-core/src/lib.rs` 现有 `pub use harness::session::{SessionActor, SessionConfig, SessionState};` 不变。

## 5. 子系统设计

### 5.1 `SessionHistory`（`session/history.rs`）

**职责**：消息历史 + 持久化 + restore + flush。

```rust
pub struct SessionHistory {
    tenant_id: String,
    session_id: String,
    entries: Vec<SessionEntry>,
    store: Option<Arc<dyn SessionStore>>,
    needs_restore: bool,
    last_saved_entry_count: usize,
    last_save: Option<tokio::task::JoinHandle<()>>,
}

impl SessionHistory {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        store: Option<Arc<dyn SessionStore>>,
    ) -> Self;

    // 消息操作
    pub fn push(&mut self, msg: AgentMessage);
    pub fn append_compaction(&mut self, entry: SessionEntry);
    pub fn truncate_before(&mut self, boundary: uuid::Uuid);

    // 读取
    pub fn messages(&self) -> Vec<AgentMessage>;
    pub fn entries(&self) -> &[SessionEntry];
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn last_compaction_timestamp(&self) -> Option<std::time::SystemTime>;

    // 持久化
    /// 首次调用时从 store 加载历史。成功后 `needs_restore = false`；
    /// 加载失败时保持 `needs_restore = true`，下次调用可重试。
    pub async fn auto_restore(&mut self) -> Result<(), AgentError>;
    pub fn persist_incremental(&mut self);
    pub fn persist_status(&self, status: &str);
    pub async fn flush(&mut self) -> Result<(), AgentError>;

    // ── 测试访问器（#[cfg(any(test, feature = "testing"))]，兼容现有 1 处私有字段访问测试）──
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn entries_mut(&mut self) -> &mut Vec<SessionEntry>;

    // 容量估算
    pub fn estimate_tokens(&self) -> usize;
}
```

**封装掉的状态**（从 SessionActor 搬走）：`entries`、`needs_restore`、`last_saved_entry_count`、`last_save`、`store`。

**改造要点**：

- 原 `persist_status` 从 `SessionActor` 方法变为 `SessionHistory::persist_status`，因为它依赖 `self.store`（搬到 history）。
- 原 `flush()` 内联在 SessionActor 的逻辑搬到 `SessionHistory::flush()`。
- 原 `last_saved_entry_count` 直接字段访问改为通过 `persist_incremental()` 内部维护。
- `auto_restore()` 内部管理 `needs_restore` 重置（与现有 `prompt_with_content` 行为一致）。

### 5.2 `SessionEventHub`（`session/event_hub.rs`）

**职责**：事件系统 + steer / follow-up 队列 + 事件处理器生命周期。

```rust
/// 事件队列中的内部包装类型。
///
/// `pub(crate)`：仅 SessionEventHub 和 SessionActor 的 `event_sink` 闭包可见。
pub(crate) struct QueuedEvent {
    pub event: AgentEvent,
}

pub struct SessionEventHub {
    listeners: Arc<Mutex<Vec<Arc<dyn AgentEventListener>>>>,
    event_tx: Option<tokio::sync::mpsc::Sender<QueuedEvent>>,
    event_processor_handle: Option<tokio::task::JoinHandle<()>>,
    steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
}

impl SessionEventHub {
    pub fn new() -> Self;

    // 事件
    pub fn emit(&self, event: AgentEvent);
    pub fn add_listener(&mut self, listener: Arc<dyn AgentEventListener>);

    // steer / follow-up
    pub fn steer(&self, msg: AgentMessage);
    pub fn follow_up(&self, msg: AgentMessage);
    pub fn drain_steer(&self) -> Vec<AgentMessage>;
    pub fn drain_follow_up(&self) -> Vec<AgentMessage>;

    // ── 内部访问器（pub(crate)，供 SessionActor 构造 AgentLoopConfig）──
    /// 克隆 event_tx，用于构造 event_sink 闭包（闭包内 try_send）。
    pub(crate) fn event_tx_clone(&self) -> Option<tokio::sync::mpsc::Sender<QueuedEvent>>;
    /// 克隆 steer_queue 的 Arc，供 AgentLoopConfig.steer_queue 借用。
    pub(crate) fn steer_queue_clone(&self) -> Arc<Mutex<Vec<AgentMessage>>>;
    /// 克隆 follow_up_queue 的 Arc，供 AgentLoopConfig.follow_up_queue 借用。
    pub(crate) fn follow_up_queue_clone(&self) -> Arc<Mutex<Vec<AgentMessage>>>;

    // 生命周期
    pub async fn shutdown(&mut self);
}

impl Drop for SessionEventHub {
    fn drop(&mut self) {
        // 同 SessionActor 现有 Drop 逻辑：drop sender 让 processor 自然退出
    }
}
```

**封装掉的状态**：`event_listeners`、`event_tx`、`event_processor_handle`、`steer_queue`、`follow_up_queue`。

**改造要点**：

- `QueuedEvent` 由 `session.rs` 私有类型转为 `event_hub.rs` 模块内 `pub(crate)` 类型。
- `spawn_event_processor` 私有方法移到 `SessionEventHub::new()` 内。
- `emit_event` → `emit`（去 `event_` 前缀，更短）。
- `Drop` impl 完全由 SessionEventHub 自己处理 processor 退出，SessionActor 不再需要管这块。
- 3 个 `pub(crate)` getter 用于 SessionActor 构造 `AgentLoopConfig`（详见 §6.2）。

### 5.3 `SessionStateMachine`（`session/state.rs`）

**职责**：状态机 + 错误原因 + 恢复 + 取消。

```rust
use crate::harness::error_recovery::RecoveryStateMachine;
use crate::SessionState;
use tokio_util::sync::CancellationToken;

pub struct SessionStateMachine {
    state: AtomicU8,                                // 0=Idle, 1=Running, 2=Error
    error_reason: Mutex<Option<String>>,
    recovery: RecoveryStateMachine,
    abort_token: CancellationToken,
}

impl SessionStateMachine {
    pub fn new(max_retries: u32) -> Self;

    // 状态转换
    pub fn enter_idle(&self);
    pub fn enter_running(&self);
    pub fn enter_error(&self, reason: String);
    pub fn clear_error(&self);

    // 读取
    pub fn state(&self) -> SessionState;
    pub fn is_streaming(&self) -> bool;
    pub fn error_reason(&self) -> Option<String>;
    pub fn recovery(&self) -> &RecoveryStateMachine;
    pub fn recovery_mut(&mut self) -> &mut RecoveryStateMachine;

    // 取消
    pub fn abort_token(&self) -> CancellationToken;
    pub fn child_token(&self) -> CancellationToken;
    pub fn abort(&self);
    pub fn reset(&mut self, max_retries: u32);

    // ── 测试访问器（#[cfg(any(test, feature = "testing"))]，兼容现有 1 处私有字段访问测试）──
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn abort_token_ref(&self) -> &CancellationToken;
}

impl Drop for SessionStateMachine {
    fn drop(&mut self) {
        self.abort_token.cancel();
    }
}
```

**封装掉的状态**：`state`、`error_reason`、`recovery`、`abort_token`。

**改造要点**：

- `SessionState` enum 仍由 `mod.rs` 定义 + re-export（保持 `agent_core::SessionState` 路径兼容）。
- `is_streaming()` / `state()` / `error_reason()` / `abort_token()` 等 SessionActor 公开方法委托。
- `RecoveryStateMachine` 字段直接嵌入（非 Arc），因为只有 SessionActor 独占。
- `abort_token_ref()` 仅在 `cfg(any(test, feature = "testing"))` 下编译，公开 API 不暴露 `&CancellationToken` 引用（避免悬垂）。

## 6. SessionActor 瘦身

### 6.1 新字段结构

```rust
pub struct SessionActor {
    // ── Identity & Config（顶层字段，被所有方法访问）──
    tenant_id: String,
    session_id: String,
    model: String,
    prompt_builder: PromptBuilder,
    base_persona: String,
    stream_options: ai_provider::StreamOptions,
    max_retries: u32,
    skills: Vec<crate::skills::Skill>,

    // ── LLM Wiring ──
    provider: Arc<dyn ai_provider::LlmProvider>,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    compaction_actor: Arc<Compactor>,
    tools: Vec<AgentToolRef>,

    // ── Strategy & Bookkeeping ──
    strategy: SessionStrategy,
    last_usage: Option<ai_provider::Usage>,

    // ── Subsystems ──
    history: SessionHistory,
    event_hub: SessionEventHub,
    state: SessionStateMachine,
}
```

**字段数**：29 → 17（含 3 个子系统）。语义上**分组明确**：8 个身份/配置 + 4 个 LLM wiring + 2 个策略 + 3 个子系统（封装了 16 个内部字段：history 7 + event_hub 5 + state 4）。

**移除字段**：

- `entries` / `needs_restore` / `last_saved_entry_count` / `last_save` / `store` → 移到 `SessionHistory`
- `event_listeners` / `event_tx` / `event_processor_handle` / `steer_queue` / `follow_up_queue` → 移到 `SessionEventHub`
- `state` / `error_reason` / `recovery` / `abort_token` → 移到 `SessionStateMachine`
- `session_started_at` → **删除**（原 `#[allow(dead_code)]`，无 reader）

### 6.2 run_with_messages 简化示例

```rust
#[instrument(skip(self), fields(tenant_id = %self.tenant_id, session_id = %self.session_id))]
async fn run_with_messages(&mut self, _add_user_msg: Option<String>)
    -> Result<Vec<AgentMessage>, AgentError>
{
    let mut all_new_msgs = Vec::new();
    loop {
        // ── 进入 Running 状态 ──
        self.state.enter_running();
        self.history.persist_status("active");
        self.event_hub.emit(AgentEvent::StateChanged { state: SessionState::Running });

        // ── 构造 AgentLoopConfig（读顶层字段 + 委托 event_hub）──
        let event_tx = self.event_hub.event_tx_clone();
        let event_sink = Arc::new(move |event: AgentEvent| {
            if let Some(tx) = &event_tx
                && tx.try_send(QueuedEvent { event }).is_err()
            {
                tracing::warn!("event queue full, dropping event");
            }
        });
        let config = AgentLoopConfig {
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            hook_dispatcher: self.hook_dispatcher.clone(),
            tools: self.tools.clone(),
            prompt_builder: self.prompt_builder.clone(),
            stream_options: self.stream_options.clone(),
            event_sink,
            steer_queue: self.event_hub.steer_queue_clone(),
            follow_up_queue: self.event_hub.follow_up_queue_clone(),
            circuit_breaker: None,
            skills: self.skills.clone(),
            text_stream_tx: None,
        };

        // ── 运行 AgentLoop ──
        let messages = SessionContextBuilder::build_context(self.history.entries());
        match AgentLoop::new(config).run(messages, self.state.child_token()).await {
            Ok(msgs) => {
                self.state.enter_idle();
                self.capture_last_usage(&msgs);
                self.apply_recovery_action(&msgs).await?;
                for msg in &msgs { self.history.push(msg.clone()); }
                all_new_msgs.extend(msgs);
            }
            Err(e) => { self.handle_loop_error(e).await?; }
        }

        // ── Mid-loop threshold compaction ──
        if self.compaction_actor.config.enabled {
            let tokens = self.history.estimate_tokens();
            let window = self.model_context_window();
            if should_compact(tokens, window, &self.compaction_actor.config) {
                self.run_auto_compaction(CompactReason::Threshold, false).await?;
            }
        }
        break;
    }

    self.history.persist_status("completed");
    self.state.clear_error();
    self.event_hub.emit(AgentEvent::StateChanged { state: SessionState::Idle });
    info!(...);
    self.history.persist_incremental();
    Ok(all_new_msgs)
}
```

**行数对比**：250 → ~80 行（去除手动 `state.store(...)`、`error_reason.lock()...`、`persist_status`、`emit_event` 模板代码）。

### 6.3 委托映射表（区分 public API 与 internal helper）

#### Public API（下游 crate 可见，签名保持不变）

| 现有 SessionActor public 方法 | 新实现（委托） |
|---|---|
| `push_message(msg)` | `self.history.push(msg)` |
| `steer(msg)` | `self.event_hub.steer(msg)` |
| `follow_up(msg)` | `self.event_hub.follow_up(msg)` |
| `messages()` | `self.history.messages()` |
| `entries()` | `self.history.entries()` |
| `flush()` | `self.history.flush()` |
| `state()` | `self.state.state()` |
| `is_streaming()` | `self.state.is_streaming()` |
| `error_reason()` | `self.state.error_reason()` |
| `abort_token()` | `self.state.abort_token()` |
| `abort()` | `self.state.abort()` |
| `reset()` | `self.state.reset(self.max_retries)` |
| `shutdown()` | `self.state.abort_token().cancel()` + `self.history.flush().await` + `self.event_hub.shutdown().await` |
| `restore()` (deprecated) | 保持空 stub |
| `last_usage()` | 直接读 `self.last_usage` 字段 |
| `add_event_listener(l)` | `self.event_hub.add_listener(l)` |
| `set_model` / `set_tools` / `set_system_prompt` / `set_stream_options` / `set_max_retries` / `set_strategy` | 直接修改顶层字段 |
| `tenant_id()` / `session_id()` / `system_prompt()` / `tools()` / `strategy()` / `abort_token()` | 直接读顶层字段 |

#### Internal helper（私有方法，不在公开 API surface）

| 现有 SessionActor 私有方法 | 新实现（委托） |
|---|---|
| `emit_event(e)` (fn, session.rs:605) | `self.event_hub.emit(e)` |
| `persist_status(s)` (fn, session.rs:1448) | `self.history.persist_status(s)` |
| `spawn_event_processor(&mut self)` (fn) | 移到 `SessionEventHub::new()` 内部 |
| `model_context_window(&self)` (fn) | 不变（纯读 provider + model 字段） |
| `apply_context_strategy_before_run(&mut self)` (fn) | 不变（操作 entries + prompt_builder，调用 self.history.entries() + self.history.truncate_before()） |

**全部 public 方法签名不变**——下游 crate 完全无需改动。

### 6.4 新增公开 API

```rust
impl SessionActor {
    pub fn history(&self) -> &SessionHistory;
    pub fn event_hub(&self) -> &SessionEventHub;
    pub fn state_machine(&self) -> &SessionStateMachine;
}
```

允许下游 crate（tavern-comp、未来 e2e tests）直接访问子系统。

## 7. 兼容性保证

| 兼容性维度 | 保证 |
|---|---|
| `SessionConfig` struct | 字段不变 |
| `BuiltSession` struct | 不变 |
| `SessionState` enum | 不变（仍由 `mod.rs` 定义 + re-export） |
| `SessionBuilder::build()` | 调用方式不变（依赖 `SessionActor::new(SessionConfig)`） |
| `dummy_for_test()` | 不变 |
| 现有 22 个 SessionActor 测试 | **测试函数体不修改**，仅当测试访问私有字段时（详见 §10）通过新增 `#[cfg(any(test, feature = "testing"))] pub(crate)` accessor 让访问路径仍然有效 |
| 现有 `SessionActor` 所有 public 方法签名 | 不变 |
| 公开 re-export (`pub use harness::session::{...}`) | 不变 |
| `session_started_at` 字段 | **移除**（无 reader，原 `#[allow(dead_code)]`） |

## 8. 迁移策略（6 阶段渐进式）

每阶段完成后跑：

```bash
cargo build -p agent-core
cargo test -p agent-core --lib harness::session
cargo build -p tavern-comp  # 验证下游仍编译
```

### 阶段 1：新建文件骨架
- 创建 `session/history.rs`、`session/event_hub.rs`、`session/state.rs`，每个放空 struct + `mod.rs` 引用
- 把 `session.rs` 重命名为 `session/old.rs` 暂时（避免与目录冲突）
- `harness/mod.rs` 改为 `pub mod session;`（自动找 `session/mod.rs`）

### 阶段 2：子系统实现
- `SessionHistory`：从 `old.rs` 剪切对应字段/方法到 `history.rs`，加 pub 修饰
- `SessionEventHub`：从 `old.rs` 剪切对应字段/方法到 `event_hub.rs`
- `SessionStateMachine`：从 `old.rs` 剪切对应字段/方法到 `state.rs`
- SessionActor 字段**保留**，但方法实现改为调用子系统（编译通过即可）

### 阶段 3：SessionActor 委托化
- 把 SessionActor 中所有相关方法改为 1-2 行委托
- 删除 SessionActor 中已迁移的字段（`entries`、`needs_restore`、`last_saved_entry_count`、`last_save`、`store`、`event_listeners`、`event_tx`、`event_processor_handle`、`steer_queue`、`follow_up_queue`、`state: AtomicU8`、`error_reason`、`recovery`、`abort_token`、`session_started_at`）
- 字段数 29 → 17（与 §6.1 一致）

### 阶段 4：测试
- 跑全部现有 22 个 SessionActor 测试（含 2 处私有字段访问测试通过新增的 `#[cfg(any(test, feature = "testing"))] pub(crate)` accessor），必须全过
- 跑 `cargo test -p agent-core --lib`，必须全过
- 跑 `cargo build -p tavern-comp`，必须全过

### 阶段 5：子系统单元测试
- 在 `history.rs` 加测试：mock store 测试 `auto_restore`、`persist_incremental`、`flush`、`entries_mut`（test accessor）
- 在 `event_hub.rs` 加测试：test listener 接收事件、`steer`/`follow_up` 队列行为
- 在 `state.rs` 加测试：状态转换原子性、`abort_token` propagation、`abort_token_ref`（test accessor）

### 阶段 6：清理
- 删除 `session/old.rs`
- 运行 `cargo clippy --workspace -- -D warnings`，确保无新增 clippy warning
- 更新 `crates/agent-core/src/harness/session/mod.rs` 顶部 doc comment
- 更新 `AGENTS.md` 当前状态表的"代码质量"行

## 9. 测试策略

### 9.1 现有测试保持不变

`session.rs` 内 22 个 `#[tokio::test]`（全部保持原样通过）：

1. `test_session_prompt`
2. `test_steer_injection`
3. `test_follow_up_loop`
4. `test_abort_session`（**访问私有字段** `session.abort_token`，通过新增 `SessionStateMachine::abort_token_ref` 兼容）
5. `test_flush_persistence`
6. `test_auto_restore_on_first_prompt`
7. `test_consecutive_prompts_persist_all_entries`
8. `test_entries_api_with_compaction`（**访问私有字段** `session.entries`，通过新增 `SessionHistory::entries_mut` 兼容）
9. `test_steer_and_follow_up_combined`
10. `test_session_hooks_are_emitted`
11. `test_multiple_prompts_increment_entries`
12. `test_concurrent_sessions_are_isolated`
13. `test_router_provider_model_context_window`
14. `test_cross_provider_model_context_window_switch`
15. `test_system_prompt_with_skills_contains_available_skills`
16. `test_set_system_prompt_preserves_skills`
17. `test_state_idle_after_creation`
18. `test_state_idle_after_successful_prompt`
19. `test_state_error_after_unrecoverable_error`
20. `test_error_state_blocks_prompt`
21. `test_reset_clears_error_state`
22. `test_reset_preserves_config`

这些测试调用 `SessionActor` 的 public API，**全部保持不变通过**。2 处私有字段访问通过新增的 `#[cfg(any(test, feature = "testing"))] pub(crate)` accessor 兼容（详见 §10）。

### 9.2 新增子系统单元测试（14 个）

| 子系统 | 测试 |
|---|---|
| `SessionHistory` | `test_history_push_and_messages`<br>`test_history_auto_restore_empty_store`<br>`test_history_auto_restore_resets_needs_restore_on_success`<br>`test_history_persist_incremental_awaits_previous`<br>`test_history_flush_blocks_on_pending_save`<br>`test_history_truncate_before_compaction` |
| `SessionEventHub` | `test_event_hub_listener_receives_events`<br>`test_event_hub_steer_drain`<br>`test_event_hub_follow_up_drain`<br>`test_event_hub_shutdown_drains_processor` |
| `SessionStateMachine` | `test_state_transitions_idle_running_idle`<br>`test_state_enter_error_clears_on_recovery`<br>`test_state_abort_propagates_to_child_token`<br>`test_state_reset_fresh_token` |

测试用 `tokio::test` + 简单 mock（不引入新依赖）。

### 9.3 集成验证

- `cargo test -p agent-core`：必须全过
- `cargo test -p tavern-comp --lib`：必须全过
- `cargo test -p api-gateway`（E2E 矩阵 9 个 suite）：必须全过
- `cargo clippy --workspace -- -D warnings`：无新增 warning

## 10. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 子系统间 borrow 冲突（如 run_with_messages 同时访问 history + event_hub + state） | Rust 允许 `self.history.x()` + `self.event_hub.y()` + `self.state.z()` 分别借用不同字段，不冲突 |
| `last_save: JoinHandle` 跨方法借用冲突 | 用 `take()` 模式（与现有代码同），所有权转移而非借用 |
| `Drop` 不能 await → background save 丢失 | 保留 `shutdown()` 强制 await，`Drop` 仅 cancel（与现有 SessionActor::drop 行为一致） |
| **2 处现有测试访问 SessionActor 私有字段**（`test_abort_session` 访问 `session.abort_token`，`test_entries_api_with_compaction` 访问 `session.entries`） | 添加 2 个 `#[cfg(any(test, feature = "testing"))] pub(crate)` accessor：`SessionStateMachine::abort_token_ref(&self) -> &CancellationToken`、`SessionHistory::entries_mut(&mut self) -> &mut Vec<SessionEntry>`。测试代码无需修改 |
| `auto_restore` 重试语义不明确 | §5.1 明确：`auto_restore()` 成功后重置 `needs_restore = false`；失败时保持 `true`，下次可重试 |
| 字段重命名导致下游 crate 编译失败 | `pub mod` re-export 全部保留 + 新增 getter 委托 |
| 阶段 2 编译错误（孤儿字段引用） | 阶段 2 仅做"剪切 + 委托"，不删除字段；阶段 3 才删除字段，确保中间状态可编译 |
| 跨 crate 访问私有字段 | 经查 `crates/tenant/src/manager.rs` 仅调用 public API（`abort_token()`、`state()`、`error_reason()`），无下游 crate 访问 SessionActor 私有字段，迁移仅影响 agent-core 内部 |

## 11. 验收标准

- [ ] `session.rs` 不再存在（被 `session/` 目录替代）
- [ ] 4 个文件：`mod.rs` ≤ 800 行、`history.rs` ≤ 300 行、`event_hub.rs` ≤ 250 行、`state.rs` ≤ 200 行
- [ ] `SessionActor` public 方法签名 100% 不变
- [ ] 现有 22 个 SessionActor 测试保持原样全过（含 2 处私有字段访问通过 `#[cfg(any(test, feature = "testing"))] pub(crate)` accessor 兼容）
- [ ] 新增 14 个子系统单元测试全过（§9.2 列名）
- [ ] `cargo test -p agent-core`、`cargo test -p tavern-comp --lib`、`cargo test -p api-gateway` 全过
- [ ] `cargo clippy --workspace -- -D warnings` 无新增 warning
- [ ] `cargo doc -p agent-core` 无 broken link