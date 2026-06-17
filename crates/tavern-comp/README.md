# tavern-comp

Tavern 是 Pandaria 的 **Agent Team 编排层**。它让多个有专业化角色的 agent 以 Pipeline、Manager-Worker 或 Ad-hoc Handoff 的方式协作完成复杂任务。

Tavern 不是通用工作流引擎。它不追求 Dify / n8n / Temporal 式的通用节点（HTTP、数据库、代码、审批 UI），而是聚焦在 **多 agent 协作协议**：角色定义、上下文隔离、显式交接、可观测性和可恢复性。

## 职责

- `Team` / `Squad` / `Role` / `Mission` 定义与校验
- `TeamContext` 协议：shared / private 上下文分离 + message thread
- `Handoff` 机制：默认继承 + 显式结构化交接
- `AgentExecutor` trait：统一接入轻量 runtime 或完整 Pandaria runtime
- Agent Team 执行引擎（`SquadEngine`，基于现有事件循环演进）
- Event Sourcing 持久化（`EventStore` trait + PG/SQLite/Memory backend）
- 执行重放（`ExecutionReplay`、`StateDiff`、`TimelineEntry`）
- Webhook 回调与 Timer 超时机制

## 公开接口

| 模块 | 核心导出 |
|---|---|
| `team` | `Team`、`Role`、`Squad`、`SquadStatus` |
| `context` | `TeamContext`、`Message`、`MessageKind`、`VisibilityRules` |
| `handoff` | `Handoff`、`AttachmentRef`、`AttachmentScope`、`HandoffMode` |
| `executor` | `AgentExecutor`、`AgentInput`、`AgentOutput`、`LocalAgentExecutor`、`PandariaAgentExecutor` |
| `workflow` | `Workflow`、`Step`、`Process`、`RouterConfig`、`InputDef`、`OutputDef`、`WebhookConfig`、`SignalTimeoutAction`（保留的旧接口） |
| `engine` | `WorkflowEngine`、`ExecutionInfo`（保留的旧接口） |
| `executor` | `StepExecutor` |
| `flow_executor` | `FlowStepExecutor` |
| `store` | `EventStore` trait、`MemoryEventStore`、`PostgreSQLEventStore`（feature `postgres`）、`SqliteEventStore`（feature `sqlite`） |
| `instance` | `InstanceState`、`InstanceStatus` |
| `replay` | `ExecutionReplay`、`ExecutionReplayer`、`ReplayOptions`、`ReplaySummary`、`StateDiff`、`TimelineEntry` |
| `validator` | `validate_dag` |
| `registry` | `WorkflowRegistry`、`WorkflowSummary` |
| `error` | `CompError` |
| `event` | `WorkflowEvent`、`SignalAction` |
| `handle` | `ExecutionHandle` |
| `timer` | `TimerRegistry` |
| `context` | `render_template` |
| `agent` | Agent 步骤桥接（Pandaria runtime 集成） |
| `hero` | Agent 注册与配置加载（将逐步迁移到 Role + AgentExecutor） |

## 编排模式

| 模式 | 说明 |
|---|---|
| **Pipeline / DAG** | 固定依赖顺序，适合确定性协作 |
| **Manager-Worker** | Manager role 动态委派任务 |
| **Ad-hoc Handoff** | Agent 输出 `Handoff` 主动决定下一步 |

三种模式共享 `TeamContext` 和 `AgentExecutor`。

## 特性开关

| Feature | 说明 |
|---|---|
| `sqlite` | SQLite EventStore backend |
| `postgres` | PostgreSQL EventStore backend |
| `bundled-sqlite` | 捆绑编译 SQLite（无需系统库） |

## 依赖

- `tavern-core` — 核心类型
- `tavern-flow-macros` — 工作流 DSL 宏
- `agent-core` — Agent 执行运行时
- `ai-provider` — LLM 通信
- `tokio` — 异步运行时
- `sqlx` — 数据库（SQLite/PostgreSQL）
- `minijinja` — 模板渲染
- `chrono` — 时间处理
- `uuid` — 唯一标识符
- `hmac` / `sha2` — Webhook 签名验证

## 边界

- **不实现**通用工作流节点（HTTP、DB、代码、审批 UI）
- **不直接调用** LLM provider，通过 `AgentExecutor` 调用 agent-core
- **不负责** tenant 调度与 quota enforcement，由 `PandariaAgentExecutor` 继承
