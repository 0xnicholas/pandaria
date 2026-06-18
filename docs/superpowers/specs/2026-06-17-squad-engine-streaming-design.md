# SquadEngine 真流式输出 — 设计文档

> 2026-06-17 | 状态：设计中

## 1. 动机

当前 `SquadEngine::run()` 是「全量返回」模式——所有 mission 执行完毕后返回一个 `SquadResult`。调用方（api-gateway → TUI / 外部 API）在 squad 执行期间看不到任何中间进度，体验上是黑盒。

本设计为 SquadEngine 增加流式输出能力，让 squad 执行过程中的生命周期事件实时推送给调用方。

## 2. 设计决策

| 决策 | 选择 |
|---|---|
| 事件粒度 | Squad/Mission 生命周期（不暴露内部 agent turn 细节） |
| API 共存 | 保留 `run()`，新增 `run_stream()`，两者共存 |
| 消费者 | TUI SSE + 外部 API |
| 持久化 | 同一事件双通道（EventStore + mpsc） |
| 流式 API 形状 | 内部 mpsc，返回 `StreamHandle`（含 Receiver + oneshot） |

## 3. 架构

```
TUI / 外部客户端
       ↑ SSE (text/event-stream)
api-gateway
  ├── tavern.rs          ← 新增 squad SSE endpoint，复用 SseStream
  ├── types.rs           ← ServerEvent 新增 squad_* 变体
  └── sse.rs             ← 无需改动
       ↑ mpsc::Receiver<ServerEvent>
tavern-comp
  └── team/engine.rs     ← 新增 run_stream()，内部双通道
       ├── EventStore::append()  (持久化，已有)
       └── mpsc::Sender          (实时推送，新增)
```

## 4. SquadEngine 改动

### 4.0 并发模型

`run_stream()` 通过 `Arc<tokio::sync::Mutex<Squad>>` 在调用方和内部 spawned task 之间共享 squad 状态。内部 spawn 一个 `tokio::task` 执行，立即返回 `StreamHandle`。

- **状态共享**：spawned task 持有 `Arc<Mutex<Squad>>` 的克隆，执行过程中写入状态变更（context 合并、completed_missions、status）。调用方在 Receiver 返回 None 后可通过同一 Arc 读取最终状态。
- **pause/resume**：pause 时 task 写入 paused status 并退出。调用方读取 paused squad 状态，持久化 completed_missions，恢复时以同一 `Arc<Mutex<Squad>>` 重新调用 `run_stream()`。
- **Receiver 关闭**：执行完成、失败、或 pause 时，内部 task 释放 `Sender`，导致 Receiver 返回 `None`。
- **清理**：spawned task 结束时通过 `oneshot` 发送 `SquadResult`。注册表管理方 await oneshot 后移除 squad 条目。

### 4.1 新增公共 API

```rust
pub struct StreamHandle {
    /// 实时事件流，squad 执行终态后关闭。
    pub events: tokio::sync::mpsc::Receiver<SquadEvent>,
    /// spawned task 结束后的最终结果（用于注册表清理）。
    pub result: tokio::sync::oneshot::Receiver<SquadResult>,
}

impl SquadEngine {
    /// 流式执行 squad，实时推送生命周期事件。
    /// 内部 spawn tokio task，通过 Arc<Mutex<Squad>> 共享状态，立即返回 StreamHandle。
    /// 通道在 squad 执行完成、失败、或 pause 后自动关闭。
    pub async fn run_stream(
        &self,
        team: &Team,
        squad: Arc<tokio::sync::Mutex<Squad>>,
    ) -> Result<StreamHandle, CompError>;
}
```

### 4.2 内部重构：提取 run_core()

`run()` 和 `run_stream()` 共享核心执行逻辑，通过 `Option<mpsc::Sender>` 区分是否流式推送。

事件先推送到 mpsc（保证实时性），然后 `EventStore::append().await?`（保证持久化）。EventStore 错误正常传播，与现有 `run()` 一致。

```rust
/// 内部共享执行核心。
/// - event_tx: Some → 事件先 try_send 到 mpsc（失败仅 warn），再 append 到 EventStore
/// - event_tx: None → 事件仅 append 到 EventStore（与现有 run() 行为一致）
async fn run_core(
    &self,
    team: &Team,
    squad: &mut Squad,
    event_tx: Option<&tokio::sync::mpsc::Sender<SquadEvent>>,
) -> Result<SquadResult, CompError>;
```

- `run()` → `run_core(team, squad, None)` — EventStore 错误正常传播
- `run_stream()` → 创建 mpsc channel，`run_core(team, squad, Some(&tx))` — 先推送后持久化，EventStore 错误正常传播

### 4.3 事件推送时机

所有事件使用现有 `SquadEvent` 枚举，不新增变体：

| 事件 | 触发点 |
|---|---|
| `SquadStarted` | run() 入口 |
| `MissionScheduled` | 调度器选出 ready mission |
| `MissionStarted` | mission 开始执行 |
| `MissionCompleted` | mission 成功 |
| `MissionFailed` | mission 失败（含重试信息） |
| `MissionWaitingForSignal` | 等待外部 signal |
| `MissionRetryScheduled` | 重试调度 |
| `SquadCompleted` | 全部完成 |
| `SquadFailed` | squad 失败 |

`SquadCreated` 在 `deploy()` 时已有持久化，不通过 `run_stream()` 推送。

### 4.4 run_dag 改造

当前并行 spawn mission 通过内部 mpsc 收集结果。改造后：

- 每个 mission spawn 时发送 `MissionStarted`
- 结果收集时发送 `MissionCompleted` / `MissionFailed`
- loop 结束时发送 `SquadCompleted` / `SquadFailed`
- pause 路径（WaitingForSignal / Breakpoint）发送对应事件后关闭通道

**`MissionFailed` 发射路径**：`execute_mission()` 新增 `event_tx: Option<&mpsc::Sender<SquadEvent>>` 参数。当重试次数耗尽时：
1. 构造 `MissionFailed { mission_id, error, attempt, will_retry: false }`
2. 若 `event_tx.is_some()`，try_send 事件
3. `EventStore::append()` 持久化
4. 返回 `Err(CompError::MissionFailed { ... })`

> 注：需要扩展 `CompError` 增加 `MissionFailed` 变体以携带 `mission_id` 和 `attempt`，供调用方后续处理。

### 4.5 mpsc 通道配置

- **容量**：256（事件队列深度；与现有 agent session SSE 通道一致）
- **并发关系**：最大并发 mission 数（`max_concurrency`，默认 4）控制同时执行的 mission 数量；256 容量保证每个 mission 产出的多个事件（Started/Completed/Failed 等）有充足缓冲
- **发送策略**：`try_send()`，失败时丢弃 + `tracing::warn`，不阻塞执行
- **关闭信号**：`drop(Sender)` → Receiver 返回 `None`

## 5. api-gateway 改动

### 5.1 ServerEvent 新增 squad 变体

```rust
pub enum ServerEvent {
    // ... 现有变体不变 ...

    #[serde(rename = "squad_started")]
    SquadStarted { squad_id: String, team_id: String },

    #[serde(rename = "squad_mission_scheduled")]
    SquadMissionScheduled { squad_id: String, mission_id: String, attempt: u64 },

    #[serde(rename = "squad_mission_started")]
    SquadMissionStarted { squad_id: String, mission_id: String },

    #[serde(rename = "squad_mission_completed")]
    SquadMissionCompleted { squad_id: String, mission_id: String, output: Value },

    #[serde(rename = "squad_mission_failed")]
    SquadMissionFailed { squad_id: String, mission_id: String, error: String, attempt: u64, will_retry: bool },

    #[serde(rename = "squad_mission_retry_scheduled")]
    SquadMissionRetryScheduled { squad_id: String, mission_id: String, attempt: u64, reason: String },

    #[serde(rename = "squad_mission_waiting_signal")]
    SquadMissionWaitingSignal { squad_id: String, mission_id: String, signal_name: String },

    #[serde(rename = "squad_completed")]
    SquadCompleted { squad_id: String, outputs: Value },

    #[serde(rename = "squad_failed")]
    SquadFailed { squad_id: String, reason: String },
}
```

`event_type_name()` 新增对应分支。

**映射说明**：`SquadEvent` 的部分变体为单元变体（`SquadStarted`、`SquadCompleted`、`SquadFailed`），映射层从执行上下文注入 `squad_id` / `team_id`。`SquadEvent::MissionCompleted` 中的 `output_key` 和 `completed_at` 对进度展示非必需，省略。

### 5.2 Squad 注册表

`TavernState` 新增内存 squad 注册表：

```rust
pub struct SquadHandle {
    pub engine: SquadEngine,
    pub squad: Arc<tokio::sync::Mutex<Squad>>,
    pub team: Team,
    /// 后台清理 task 的 AbortHandle。
    _cleanup: tokio::task::AbortHandle,
}

pub struct TavernState {
    // ... 现有字段 ...
    pub squads: Arc<RwLock<HashMap<String, SquadHandle>>>,
}
```

**生命周期：**
- **注册**：`deploy()` 成功后，插入 `squads` map（key = squad_id）
- **淘汰**：spawn 一个后台 task 等待 `StreamHandle::result` oneshot，收到结果后从 map 移除条目。`SquadHandle` 持有该 task 的 `AbortHandle`（SSE 断开时可用于提前清理）
- **pause 场景**：pause 时 spawned task 退出并发送 `SquadResult`（含 paused status）。清理 task 不移除条目——squad 在 pause 期间保留在注册表中，等待 resume

### 5.3 SSE 端点

新增路由：`GET /tavern/squads/{squad_id}/events/stream`

Handler 逻辑：
1. 从 `TavernState.squads` 查找 `SquadHandle`
2. 调用 `handle.engine.run_stream(&handle.team, handle.squad.clone())` 获取 `StreamHandle`
3. spawn task 将 `SquadEvent` 映射为 `ServerEvent`，送入 mpsc
4. 返回 `SseStream`（复用现有 `sse.rs`）

> 认证/限流：继承 api-gateway 现有中间件（HMAC 认证、rate limit），无需额外配置。

> 注：`Squad` 含 `Arc<dyn AgentExecutor>` 不可序列化，因此 squad 查找走内存注册表而非 EventStore 恢复。

### 5.4 TUI 客户端

TUI 新增 squad 事件类型匹配：
- `client/` SSE 连接逻辑扩展 squad 事件类型
- squad 执行视图展示实时 mission 进度（mission 列表 + 状态标记）

## 6. 数据流示例

DAG 模式下 3 个 mission（a, b 并行；c 依赖 a, b）：

```
run_stream() 被调用
  ├─ emit SquadStarted
  ├─ scheduler.ready() → [a, b]
  ├─ emit MissionScheduled(a), MissionScheduled(b)
  ├─ spawn a → emit MissionStarted(a)
  ├─ spawn b → emit MissionStarted(b)
  ├─ a 完成 → emit MissionCompleted(a)
  ├─ b 完成 → emit MissionCompleted(b)
  ├─ scheduler.ready() → [c]
  ├─ emit MissionScheduled(c)
  ├─ spawn c → emit MissionStarted(c)
  ├─ c 完成 → emit MissionCompleted(c)
  ├─ emit SquadCompleted
  └─ drop(Sender) → 通道关闭
```

并行 mission 的事件可能交错到达（a 和 b 的 Started 可连续到达）。

## 7. 错误处理

| 场景 | 行为 |
|---|---|
| mpsc channel full | `try_send` 失败 → 丢弃事件 + `tracing::warn`，不阻塞执行 |
| 消费者提前断开 | `try_send` 返回 `Err` → 忽略，执行继续 |
| mission 执行失败 | 推送 `MissionFailed`；若不可重试则推送 `SquadFailed` + 关闭通道 |
| EventStore append 失败 | 事件先推送到 mpsc（保证实时性），然后 `EventStore::append().await?` 错误正常传播。注意：若 store append 失败，消费者已收到的事件未被持久化——这是实时性优先的 trade-off。重启后以 EventStore 为真相源 |
| squad 中途 pause | 推送对应 pause 事件后关闭通道；恢复时调用方重新 `run_stream()` |

## 8. 测试策略

### tavern-comp 单元测试

1. 单 mission squad：验证完整事件序列
2. 3 个并行 mission：验证事件交错顺序
3. mission 失败：验证 MissionFailed + SquadFailed
4. 通道容量：64 个并发 mission（远大于默认 max_concurrency=4），验证无死锁无 panic（通道容量 256 足够缓冲）
5. 消费者断开：提前 drop Receiver，验证执行不受影响
6. `run()` 向后兼容：现有行为不变
7. pause/resume 流式：mission 触发 WaitingForSignal → 验证事件序列 + Receiver 关闭 → 读取 Arc<Mutex<Squad>> 状态 → 重新 run_stream() → 验证恢复后事件序列

### tavern-comp 集成测试

- `stream_events_dag_parallel`：DAG 模式流式事件完整性
- `stream_events_hierarchical`：hierarchical 模式
- `stream_events_pause_resume`：pause/resume 状态保持

### api-gateway 层

- `SquadEvent → ServerEvent` 映射覆盖
- SSE endpoint Content-Type 校验
- E2E：创建 squad → 订阅 SSE → 验证事件序列

### TUI 层

- SSE 事件分支匹配 snapshot 测试

## 9. 不改动范围

- `SquadEvent` 枚举（类型定义不变，但新增发射点——现有代码从未 emit `MissionFailed`）
- `EventStore` trait 和实现
- `AgentExecutor` trait
- `run()` 方法签名和行为
- `sse.rs` 基础设施

需要扩展的类型：
- `CompError` 新增 `MissionFailed { mission_id: String, attempt: u64, source: Box<CompError> }` 变体（携带 mission 元数据供事件构造）

## 10. 实施文件清单

| 文件 | 改动类型 |
|---|---|
| `crates/tavern-comp/src/team/engine.rs` | 重构 + 新增 `run_core()`, `run_stream()`, `StreamHandle` |
| `crates/tavern-comp/src/team/engine.rs` (tests) | 新增单元测试 |
| `crates/tavern-comp/src/error.rs` | 新增 `CompError::MissionFailed` 变体 |
| `crates/tavern-comp/tests/squad_integration.rs` | 新增流式集成测试 |
| `crates/api-gateway/src/types.rs` | 新增 squad ServerEvent 变体 + 映射 |
| `crates/api-gateway/src/tavern.rs` | 新增 squad SSE handler + 路由 + `SquadHandle` 类型 + 注册表管理 |
| `crates/api-gateway/tests/` | 新增 SSE E2E 测试 |
| TUI `client/` | SSE 事件扩展 + squad 进度 UI |
