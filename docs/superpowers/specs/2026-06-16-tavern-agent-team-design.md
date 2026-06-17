# Tavern Agent Team 设计

> 状态：设计已确认，待实现  
> 作者：pi / Nicholas  
> 日期：2026-06-16

---

## 1. 背景与问题

Tavern 目前被描述为"工作流引擎"，但代码中节点类型高度偏向 agent 执行（`agent_id + task`），对通用工作流所需的 HTTP、数据库、代码、审批 UI 等节点支持很弱。继续以"通用工作流引擎"定位会导致：

- 用户期望与实际能力不匹配。
- 与 Dify / n8n / Temporal 等产品竞争时缺乏通用节点优势。
- 核心抽象被摊薄，难以做深。

与此同时，Pandaria 核心（`agent-core`）已经提供了**单个 agent 的完整 runtime**：tool-use loop、session 隔离、quota、hook、memory、持久化。Tavern 应该在此基础上解决更高层的问题：**如何让多个专业化 agent 协作完成复杂任务**。

### 1.1 核心洞察

"多个 agent 按顺序调用" 和 "Agent Team" 的区别：

| 维度 | 多 agent 顺序调用 | Agent Team |
|---|---|---|
| 角色 | 临时指定 agent_id | 固定 Role，有 instructions、skills、模型、可见性 |
| 上下文 | 单个大 context | shared / private 分离，有 message thread |
| 交接 | 输出自动继承 | 默认继承 + 显式 Handoff |
| 决策 | 人写死依赖 | 可静态编排，也可由 Manager/Agent 动态 handoff |
| 可观测性 | 单 agent 级别 | Team / Role / Handoff 聚合 |
| 可恢复性 | 无 | 事件溯源 + Squad 状态恢复 |

Agent Team 是真实存在的产品需求，不是伪概念。

---

## 2. 定位声明

> **Tavern 是 Pandaria 的 Agent Team 编排层。**  
> 它让多个有专业化角色的 agent 在受控、可观测、可恢复的方式下协作完成复杂任务。  
> 它不是通用工作流引擎，不追求 Dify/n8n 式的通用节点生态。

### 2.1 职责边界

| 属于 Tavern | 不属于 Tavern |
|---|---|
| Team / Squad / Role / Mission 定义 | 通用 HTTP/DB/代码/审批 UI 节点 |
| Agent Team 编排协议（context、handoff） | Tenant 调度、配额 enforcement |
| `AgentExecutor` 抽象 | LLM provider 通信细节 |
| 事件溯源、replay、signal、retry | Session 内部 memory compaction |
| 多 agent 协作的可观测性聚合 | 单 agent 的 tool execution |

### 2.2 与 Pandaria 的关系

```
┌─────────────────────────────────────────┐
│            api-gateway / tui            │
└─────────────────────────────────────────┘
                   │
┌─────────────────────────────────────────┐
│              Tavern Layer               │
│   Team, Squad, Mission, Handoff,        │
│   TeamContext, AgentExecutor            │
└─────────────────────────────────────────┘
                   │
┌─────────────────────────────────────────┐
│            agent-core / tenant          │
│   SessionActor, AgentLoop, ToolExecutor │
│   HookDispatcher, Quota, Persistence    │
└─────────────────────────────────────────┘
```

Tavern 通过 `AgentExecutor` trait 调用 agent-core，不直接操作 LLM provider。

---

## 3. 核心抽象

### 3.1 Team

可复用的团队定义：一组角色 + 共享目标 + 默认编排模式。

```rust
pub struct Team {
    /// 全局唯一标识符，约束：^[a-zA-Z0-9_-]+$
    pub id: String,

    /// 可读名称
    pub name: String,

    /// 团队描述
    pub description: Option<String>,

    /// 角色列表
    pub roles: Vec<Role>,

    /// 默认编排模式
    pub default_process: Process,

    /// 可选：LLM planning 配置
    pub planning: Option<PlanningConfig>,

    /// 可选：完成/失败时的 webhook
    pub webhook: Option<WebhookConfig>,
}
```

### 3.2 Role

Team 中的角色。Role 与 agent-core 的 `AgentConfig` 是不同抽象：

- `Role` 是 team 内视角：你在这个 team 里扮演什么、能看到什么、默认用什么模型。
- `AgentConfig` 是 agent-core 视角：一个 agent 的完整配置。

```rust
/// P0 直接复用 tavern-core 的 SkillConfig；未来可抽象为更轻的 SkillRef。
pub type SkillRef = SkillConfig;

pub struct Role {
    pub id: String,
    pub name: String,
    pub description: Option<String>,

    /// 映射到 agent-core AgentConfig 的 id
    pub agent_id: String,

    /// 团队内额外 instructions，会追加到 AgentConfig.instructions
    pub team_instructions: Option<String>,

    /// 该 role 默认覆盖的模型
    pub model_override: Option<ModelConfig>,

    /// 可见性配置：默认能读哪些 scope
    pub visibility: Visibility,

    /// 该 role 可使用的 skills（追加/覆盖）
    pub skills: Vec<SkillRef>,
}

pub struct Visibility {
    /// 是否可读 shared
    pub read_shared: bool,
    /// 默认可读的其他 role private 空间
    pub read_private_roles: Vec<String>,
}
```

### 3.3 Squad

一次具体的执行实例，= `Team` + 输入 + runtime 状态。

```rust
pub struct Squad {
    pub id: String,
    pub team_id: String,
    pub status: SquadStatus,
    pub context: TeamContext,
    pub executor: Arc<dyn AgentExecutor>,
}

pub enum SquadStatus {
    Pending,
    Running,
    WaitingForSignal { signal: String },
    Sleeping { wake_at: DateTime<Utc> },
    Completed,
    Failed,
}
```

> `Squad` 替代当前 `Workflow` 作为面向用户的执行单元。`Workflow` 保留为底层编排描述，但用户主要操作 `Team` / `Squad`。

### 3.4 Mission

一次要交给某个 role 执行的任务。对应现有 `Step`，但绑定到 `role`。

```rust
pub struct Mission {
    pub id: String,

    /// 引用 Team.roles 中的 role id
    pub role: String,

    /// 任务模板，支持 {{var}} 插值
    pub task: String,

    /// AND 依赖
    pub depends_on: Vec<String>,

    /// OR 依赖（任一上游完成即触发）
    pub or_depends_on: Vec<String>,

    /// 默认继承模式下的输出键名
    pub output_key: Option<String>,

    /// handoff 模式
    pub handoff_mode: HandoffMode,

    // 现有字段保留
    pub timeout: Option<u64>,
    pub retries: Option<u64>,
    pub retry_delay: Option<u64>,
    pub wait_for_signal: Option<String>,
    pub signal_timeout: Option<u64>,
    pub signal_timeout_action: Option<SignalTimeoutAction>,
    pub breakpoint: bool,
}

pub enum HandoffMode {
    /// 普通输出按 output_key 进入 shared
    Inherit,
    /// agent 必须输出 Handoff 结构
    Required,
    /// 自动识别：普通值走 Inherit，符合 Handoff schema 走 Handoff
    Auto,
}
```

### 3.5 AgentExecutor

agent 执行的抽象边界，让 Tavern 能同时接入测试/轻量 runtime 和生产级 Pandaria runtime。

```rust
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    /// 解析 role 对应的完整 agent 配置
    async fn resolve_role(&self, role_id: &str) -> Result<Role, AgentExecutorError>;

    /// 执行一次 agent 调用
    async fn execute(
        &self,
        role_id: &str,
        input: AgentInput,
    ) -> Result<AgentOutput, AgentExecutorError>;

    /// 可选：流式执行
    async fn execute_stream(
        &self,
        role_id: &str,
        input: AgentInput,
    ) -> Result<BoxStream<'static, AgentOutputChunk>, AgentExecutorError>;
}

pub struct AgentInput {
    pub task: String,
    pub context: TeamContext,
    pub model_override: Option<ModelConfig>,
    pub timeout: Option<Duration>,
}

pub struct AgentOutput {
    pub content: Value,
    pub usage: Option<Usage>,
    pub latency: Duration,
    pub metadata: HashMap<String, Value>,
}
```

两个内置实现：

| 实现 | 用途 |
|---|---|
| `LocalAgentExecutor` | 测试、轻量工具、本地模式，保留现有 `AgentRuntime` 能力 |
| `PandariaAgentExecutor` | 生产环境，每次 execute 启动/复用一个 `SessionActor`，继承 tenant/session/quota/hook |

### 3.6 TeamContext

团队级上下文，解决 agent 之间"看见什么、记住什么、交接什么"。

```rust
pub struct TeamContext {
    /// 团队共享上下文：任何 role 默认可读
    pub shared: Value,

    /// 角色私有上下文：每个 role 有自己的写入空间
    pub private: HashMap<String, Value>,

    /// 消息线程：记录每次调用、输出、handoff、外部事件
    pub thread: Vec<Message>,

    /// 可见性规则
    pub visibility: VisibilityRules,
}

pub struct Message {
    pub id: String,
    pub role: String,
    pub turn: u32,
    pub kind: MessageKind,
    pub content: Value,
    pub timestamp: DateTime<Utc>,
}

pub enum MessageKind {
    Invocation,
    Output,
    Handoff,
    Observation,
    System,
}

pub struct VisibilityRules {
    /// role id -> 该 role 可读的其他 role private 空间
    pub role_can_read: HashMap<String, Vec<String>>,
}
```

#### private / shared 规则

- agent 被调用时：
  - 可读 `shared`
  - 可读自己的 `private[role]`
  - 可读 `visibility.role_can_read[role]` 中授权的其他 role private 空间
- agent 输出时：
  - 普通输出按 `output_key` 进入 `shared`
  - 结构化输出可写入自己的 `private`（约定键名或 schema）
- `Handoff.attachments` 可以显式把 private 内容授权给 `next_role`

#### 模板解析优先级

现有 `{{var}}` 模板仍然工作：

1. `shared` 中的键
2. 当前 role 的 `private` 中的键
3. 被授权访问的其他 role private 中的键
4. `thread` 中最近一条消息的内容（作为 `_last_message`）

---

## 4. Handoff 机制

### 4.1 默认继承模式

agent 输出普通 JSON / string 时，按 `output_key` 进入 `shared`。

```yaml
missions:
  - id: research
    role: researcher
    task: "研究 {{topic}}"
    output_key: research_notes
```

`research_notes` 写入 `TeamContext.shared`。

### 4.2 显式 Handoff 模式

agent 输出符合 schema 的结构化对象时，引擎解析并执行交接。

```rust
pub struct Handoff {
    /// 本次产出摘要
    pub summary: String,

    /// 明确要交接给的角色
    pub next_role: Option<String>,

    /// 候选角色（让 manager/引擎决定）
    pub candidates: Vec<String>,

    /// 给下一个 role 的 instructions
    pub instructions: Option<String>,

    /// 要随交接共享的内容引用
    pub attachments: Vec<AttachmentRef>,

    /// 产出数据
    pub payload: Value,

    /// 是否请求人工介入
    pub request_human: bool,

    /// 是否终止 squad
    pub terminate: bool,
}

pub struct AttachmentRef {
    pub scope: AttachmentScope,
    pub key: String,
}

pub enum AttachmentScope {
    Shared,
    Private { role: String },
}
```

### 4.3 引擎处理 Handoff 的逻辑

1. `payload` 进入 `shared`（或按 `output_key` 命名）
2. `summary`、`instructions` 写入 `thread` 作为 `MessageKind::Handoff`
3. `attachments` 按可见性规则提升到 shared 或临时授权给 `next_role`
4. 若 `request_human=true`，squad 进入 `WaitingForSignal`
5. 若 `terminate=true`，squad 以当前 `shared` 完成
6. 否则根据 `next_role` / `candidates` 决定下一个 role

### 4.4 不同编排模式下的 Handoff

| 模式 | Handoff 用法 |
|---|---|
| Pipeline / DAG | 默认继承；关键节点可显式覆盖下一步 |
| Manager-Worker | Manager role 输出 Handoff 指定 `next_role` 和 `instructions` |
| Ad-hoc 协作 | Agent 自主输出 Handoff，动态决定下一步 |

---

## 5. 编排模式

三种模式共享 `TeamContext` 和 `AgentExecutor`。

### 5.1 Pipeline / DAG

固定依赖，适合确定性协作。沿用现有 DAG 校验和事件循环。

### 5.2 Manager-Worker

Manager role 动态委派任务。Manager 输出 `Handoff` 决定下一步。

### 5.3 Ad-hoc Handoff

任何 agent 都可以输出 `Handoff` 主动决定下一步。这是最灵活的协作模式，但也最需要可见性规则约束。

---

## 6. Pandaria 集成

### 6.1 PandariaAgentExecutor

```rust
pub struct PandariaAgentExecutor {
    tenant_id: String,
    session_store: Arc<dyn SessionStore>,
    agent_loop_factory: Arc<dyn AgentLoopFactory>,
    hook_dispatcher: Arc<dyn HookDispatcher>,
}
```

执行时：

1. 创建或复用一个 `SessionActor`
2. 把 `AgentInput.task` 和 `TeamContext` 摘要作为 user message
3. 运行 `AgentLoop`
4. 返回最终 assistant message 作为 `AgentOutput.content`
5. 返回 `Usage` 和 `latency` 给 Tavern 做 team 级聚合

> 初版设计为每次 execute 一次 SessionActor，后续可优化为同一 Squad 内 session 复用。

### 6.2 继承的能力

通过 `PandariaAgentExecutor`，Agent Team 自动获得：

- tenant/session 隔离
- 并发/配额控制
- HookDispatcher 策略
- memory / compaction
- 持久化与恢复
- token/tool call 计量
- 全链路 tracing

---

## 7. 可观测性

### 7.1 聚合维度

- Team 总 token 消耗
- 每个 role 的调用次数、成功率、latency
- Handoff 次数与路径
- 等待 signal 的时间
- Squad 整体耗时与状态流转

### 7.2 Tracing 要求

所有 span 必须携带：

- `tenant_id`
- `squad_id`
- `team_id`
- `role_id`
- `mission_id`

---

## 8. 向后兼容与迁移

### 8.1 保留现有 API

- `Workflow` / `WorkflowEngine` / `Step` / `Instance` 保留一个版本
- 现有测试和 api-gateway 路由不立即破坏

### 8.2 新增 API

- `Team` / `Squad` / `Mission` / `Role` / `AgentExecutor` / `TeamContext` / `Handoff`
- 新增 `SquadEngine` 替代/封装 `WorkflowEngine`

### 8.3 迁移路径

| 旧概念 | 新概念 |
|---|---|
| `Workflow` | `Team`（概念）+ `Workflow`（底层编排描述） |
| `Step` | `Mission` |
| `Instance` | `Squad` |
| `TavernHero` | `AgentExecutor` + `RoleRegistry` |
| `output_key` | 默认继承模式 |
| `router` | `Handoff` + 静态 DAG router |

---

## 9. 实施路线图

| Phase | 内容 |
|---|---|
| P0 | 定义 `AgentExecutor`、`TeamContext`、`Handoff`、`Role`、`Team`、`Squad` 类型 |
| P1 | 把现有 `WorkflowEngine` 核心逻辑迁移到 `SquadEngine`，复用事件存储/replay |
| P2 | 实现 `LocalAgentExecutor` 和 `PandariaAgentExecutor` |
| P3 | 用新抽象重写核心 interpreter 测试 |
| P4 | 更新 api-gateway 的 `/tavern` 路由到 Team/Squad 语义 |
| P5 | 迁移 proc-macro DSL |

---

## 10. 已废弃/清理的说法

以下说法从 Tavern 文档中移除：

- "通用工作流引擎"
- "Dify/n8n 式工作流"
- "HTTP/数据库/代码节点"（未实现，不应承诺）

替换为：

- "Agent Team 编排层"
- "多 agent 协作框架"
- "角色化、可观测、可恢复的 agent 协作"

---

## 11. 相关文档

- `crates/tavern-core/README.md`
- `crates/tavern-comp/README.md`
- `crates/tavern-flow-macros/README.md`
- `AGENTS.md` 中关于 Tavern 的章节
