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
| 流式 API 形状 | 内部 mpsc，返回 `Receiver<SquadEvent>` |

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

### 4.1 新增公共 API

```rust
impl SquadEngine {
    /// 流式执行 squad，实时推送生命周期事件。
    /// 返回 mpsc Receiver，通道在 squad 执行完成、失败、或 pause 后自动关闭。
    pub async fn run_stream(
        &self,
        team: &Team,
        squad: &mut Squad,
    ) -> Result<tokio::sync::mpsc::Receiver<SquadEvent>, CompError>;
}
```

### 4.2 内部重构：提取 run_core()

`run()` 和 `run_stream()` 共享核心执行逻辑，通过回调注入事件分发：

```rust
async fn run_core<F>(
    &self,
    team: &Team,
    squad: &mut Squad,
    emit: F,
) -> Result<SquadResult, CompError>
where
    F: Fn(SquadEvent) + Send + Sync + Clone + 'static;
```

- `run()` → `run_core(team, squad, |e| { self.store.append(e); })`（仅持久化）
- `run_stream()` → 创建 mpsc channel，`run_core(team, squad, |e| { let _ = tx.try_send(e); self.store.append(e); })`（持久化 + 推送）

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

### 4.5 mpsc 通道配置

- **容量**：256（与现有 agent session SSE 通道一致）
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

    #[serde(rename = "squad_mission_waiting_signal")]
    SquadMissionWaitingSignal { squad_id: String, mission_id: String, signal_name: String },

    #[serde(rename = "squad_completed")]
    SquadCompleted { squad_id: String, outputs: Value },

    #[serde(rename = "squad_failed")]
    SquadFailed { squad_id: String, reason: String },
}
```

`event_type_name()` 新增对应分支。

### 5.2 SSE 端点

新增路由：`GET /tavern/squads/{squad_id}/events/stream`

Handler 逻辑：
1. 从 EventStore 恢复 squad 上下文
2. 调用 `squad_engine.run_stream()` 获取 `Receiver<SquadEvent>`
3. spawn task 将 `SquadEvent` 映射为 `ServerEvent`，送入 mpsc
4. 返回 `SseStream`（复用现有 `sse.rs`）

### 5.3 TUI 客户端

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
| EventStore append 失败 | 先推送事件，EventStore 错误通过 `tracing::error` 记录 |
| squad 中途 pause | 推送对应 pause 事件后关闭通道；恢复时调用方重新 `run_stream()` |

## 8. 测试策略

### tavern-comp 单元测试

1. 单 mission squad：验证完整事件序列
2. 3 个并行 mission：验证事件交错顺序
3. mission 失败：验证 MissionFailed + SquadFailed
4. 通道容量：64 并发，验证无死锁无 panic
5. 消费者断开：提前 drop Receiver，验证执行不受影响
6. `run()` 向后兼容：现有行为不变

### tavern-comp 集成测试

- `stream_events_dag_parallel`：DAG 模式流式事件完整性
- `stream_events_hierarchical`：hierarchical 模式

### api-gateway 层

- `SquadEvent → ServerEvent` 映射覆盖
- SSE endpoint Content-Type 校验
- E2E：创建 squad → 订阅 SSE → 验证事件序列

### TUI 层

- SSE 事件分支匹配 snapshot 测试

## 9. 不改动范围

- `SquadEvent` 枚举
- `EventStore` trait 和实现
- `AgentExecutor` trait
- `run()` 方法签名和行为
- `sse.rs` 基础设施

## 10. 实施文件清单

| 文件 | 改动类型 |
|---|---|
| `crates/tavern-comp/src/team/engine.rs` | 重构 + 新增 `run_core()`, `run_stream()` |
| `crates/tavern-comp/src/team/engine.rs` (tests) | 新增单元测试 |
| `crates/tavern-comp/tests/squad_integration.rs` | 新增流式集成测试 |
| `crates/api-gateway/src/types.rs` | 新增 squad ServerEvent 变体 + 映射 |
| `crates/api-gateway/src/tavern.rs` | 新增 squad SSE handler + 路由 |
| `crates/api-gateway/tests/` | 新增 SSE E2E 测试 |
| TUI `client/` | SSE 事件扩展 + squad 进度 UI |
