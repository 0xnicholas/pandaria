# tavern-comp

Tavern 工作流编排引擎。基于 DAG 的多 Agent 工作流执行系统，支持 Event Sourcing、执行重放、Webhook 和 Timer。

## 职责

- 工作流定义与校验（`Workflow`、DAG 循环检测）
- 工作流执行引擎（`WorkflowEngine`）
- 步骤执行器（`StepExecutor`、`FlowStepExecutor`）
- Event Sourcing 持久化（`EventStore` trait + PG/SQLite/Memory backend）
- 执行重放（`ExecutionReplay`、`StateDiff`、`TimelineEntry`）
- 工作流实例状态跟踪（`InstanceStatus`）
- Webhook 回调与 Timer 超时机制

## 公开接口

| 模块 | 核心导出 |
|---|---|
| `workflow` | `Workflow`、`Step`、`Process`、`RouterConfig`、`InputDef`、`OutputDef`、`WebhookConfig`、`SignalTimeoutAction` |
| `engine` | `WorkflowEngine`、`ExecutionInfo` |
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
| `hero` | Agent 注册与配置加载 |

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
