# Tavern Agent Team P1 SquadEngine 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `tavern-comp` 中实现 `SquadEngine`，支持 `Team` 的 Sequential 编排模式：按 mission 定义顺序串行执行，agent 输出按 `output_key` 写入 `TeamContext.shared`，为 P2 的 DAG/Hierarchical 打基础。

**Architecture:** 新增 `crates/tavern-comp/src/team/engine.rs` 作为 `SquadEngine`；复用 `AgentExecutor` 执行 mission；通过 `TeamContext` 传递共享上下文；事件持久化复用 `EventStore`，新增 `WorkflowEvent::SquadEventOccurred` 包装 `SquadEvent`；测试使用 `MockAgentExecutor`。

**Tech Stack:** Rust, tokio, serde_json, chrono, async-trait

---

## 范围说明

P1 刻意不做：
- DAG 拓扑排序（mission 按 YAML 定义顺序执行，依赖仅在测试中用断言保证）
- `or_depends_on` / Router / Hierarchical
- `wait_for_signal` / `breakpoint` / retry timer
- Handoff 附件传播（只记录到 thread）
- 真实 Pandaria 集成（P2 做 `PandariaAgentExecutor`）

P1 必须做：
- `Team` 添加 `missions` 字段
- `Role` / `Mission` 添加 `Default`
- `SquadEngine::deploy` + `SquadEngine::run`
- `TeamRegistry`
- `SquadEvent` + `WorkflowEvent::SquadEventOccurred`
- 端到端 Sequential 测试

---

## 文件结构

- **Create:** `crates/tavern-comp/src/team/engine.rs` — `SquadEngine`
- **Create:** `crates/tavern-comp/src/team/registry.rs` — `TeamRegistry`
- **Modify:** `crates/tavern-comp/src/team/mod.rs` — 导出 `SquadEngine`, `TeamRegistry`, `SquadResult`
- **Modify:** `crates/tavern-comp/src/team/squad.rs` — 添加 `SquadResult` 和 `Squad::new`
- **Modify:** `crates/tavern-comp/src/team/definition.rs` — 添加 `missions` 字段和 `Team::validate`
- **Modify:** `crates/tavern-comp/src/team/role.rs` — 添加 `Default` derive
- **Modify:** `crates/tavern-comp/src/team/mission.rs` — 添加 `Default` derive
- **Modify:** `crates/tavern-comp/src/team/executor.rs` — 添加 `MockAgentExecutor`
- **Modify:** `crates/tavern-comp/src/event.rs` — 添加 `SquadEvent` 和 `WorkflowEvent::SquadEventOccurred`
- **Modify:** `crates/tavern-comp/src/error.rs` — 添加 `TeamNotFound`, `RoleNotFound`, `MissionNotFound`, `SquadNotFound`, `DuplicateTeam`
- **Modify:** `crates/tavern-comp/src/replay.rs` — 处理新增 `WorkflowEvent::SquadEventOccurred` 分支
- **Modify:** `crates/tavern-comp/src/lib.rs` — 导出新类型

---

## Task 1: 给 Role 和 Mission 添加 Default

**Files:**
- Modify: `crates/tavern-comp/src/team/role.rs`
- Modify: `crates/tavern-comp/src/team/mission.rs`

- [ ] **Step 1: `role.rs`**

将 `Role` derive 改为：

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Role {
```

- [ ] **Step 2: `mission.rs`**

将 `Mission` derive 改为：

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mission {
```

- [ ] **Step 3: 运行测试**

```bash
cargo test -p tavern-comp --lib team::role::tests team::mission
```

Expected: PASS

---

## Task 2: Team 添加 missions 字段

**Files:**
- Modify: `crates/tavern-comp/src/team/definition.rs`

- [ ] **Step 1: 修改 `Team` 结构体**

```rust
use serde::{Deserialize, Serialize};
use tavern_core::PlanningConfig;

use crate::error::CompError;
use crate::workflow::{Process, WebhookConfig};
use super::mission::Mission;
use super::role::Role;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub roles: Vec<Role>,
    #[serde(default)]
    pub missions: Vec<Mission>,
    #[serde(default)]
    pub default_process: Process,
    #[serde(default)]
    pub planning: Option<PlanningConfig>,
    #[serde(default)]
    pub webhook: Option<WebhookConfig>,
}
```

- [ ] **Step 2: 添加 `Team::validate`**

```rust
impl Team {
    pub fn validate(&self) -> Result<(), CompError> {
        if !tavern_core::is_valid_id(&self.id) {
            return Err(CompError::ConfigParse {
                path: "<team>".into(),
                reason: format!("invalid team id '{}'", self.id),
            });
        }
        if self.roles.is_empty() {
            return Err(CompError::ConfigParse {
                path: "<team>".into(),
                reason: "team must have at least one role".into(),
            });
        }
        let mut seen = std::collections::HashSet::new();
        for role in &self.roles {
            if !seen.insert(role.id.clone()) {
                return Err(CompError::ConfigParse {
                    path: "<team>".into(),
                    reason: format!("duplicate role id '{}'", role.id),
                });
            }
            if role.agent_id.trim().is_empty() {
                return Err(CompError::ConfigParse {
                    path: "<team>".into(),
                    reason: format!("role '{}' has empty agent_id", role.id),
                });
            }
        }
        if !matches!(self.default_process, Process::Sequential) {
            return Err(CompError::ConfigParse {
                path: "<team>".into(),
                reason: "P1 only supports sequential process".into(),
            });
        }
        Ok(())
    }

    pub fn missions(&self) -> &[Mission] {
        &self.missions
    }
}
```

- [ ] **Step 3: 更新测试**

将 `team_yaml_deserialize` 中 YAML 加入 `missions: []` 或依赖 `#[serde(default)]`。

添加验证测试：

```rust
#[test]
fn team_validate_duplicate_role() {
    let team = Team {
        id: "t1".into(),
        name: "test".into(),
        description: None,
        roles: vec![
            Role { id: "r1".into(), name: "R1".into(), agent_id: "a1".into(), ..Default::default() },
            Role { id: "r1".into(), name: "R2".into(), agent_id: "a2".into(), ..Default::default() },
        ],
        missions: vec![],
        default_process: Process::Sequential,
        planning: None,
        webhook: None,
    };
    assert!(team.validate().is_err());
}
```

- [ ] **Step 4: 运行测试**

```bash
cargo test -p tavern-comp --lib team::definition::tests
```

Expected: PASS

---

## Task 3: 扩展事件类型

**Files:**
- Modify: `crates/tavern-comp/src/event.rs`

- [ ] **Step 1: 添加 `SquadEvent`**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SquadEvent {
    SquadCreated {
        squad_id: String,
        team_id: String,
        inputs: Value,
    },
    SquadStarted,
    MissionScheduled {
        mission_id: String,
        attempt: u64,
    },
    MissionStarted {
        mission_id: String,
        started_at: DateTime<Utc>,
    },
    MissionCompleted {
        mission_id: String,
        output: Value,
        output_key: Option<String>,
        completed_at: DateTime<Utc>,
    },
    MissionFailed {
        mission_id: String,
        error: String,
        attempt: u64,
        will_retry: bool,
    },
    SquadCompleted {
        outputs: Value,
        completed_at: DateTime<Utc>,
    },
    SquadFailed {
        reason: String,
        failed_at: DateTime<Utc>,
    },
}
```

- [ ] **Step 2: 在 `WorkflowEvent` 添加包装变体**

```rust
SquadEventOccurred {
    event: SquadEvent,
    occurred_at: DateTime<Utc>,
},
```

- [ ] **Step 3: 添加 `impl From<SquadEvent> for WorkflowEvent`**

```rust
impl From<SquadEvent> for WorkflowEvent {
    fn from(event: SquadEvent) -> Self {
        WorkflowEvent::SquadEventOccurred {
            event,
            occurred_at: Utc::now(),
        }
    }
}
```

- [ ] **Step 4: 运行测试**

```bash
cargo test -p tavern-comp --lib event::
```

Expected: PASS

---

## Task 4: 扩展错误类型

**Files:**
- Modify: `crates/tavern-comp/src/error.rs`

- [ ] **Step 1: 添加变体**

```rust
#[error("team '{id}' not found")]
TeamNotFound { id: String },

#[error("role '{id}' not found in team")]
RoleNotFound { id: String },

#[error("mission '{id}' not found in squad")]
MissionNotFound { id: String },

#[error("squad '{id}' not found")]
SquadNotFound { id: String },

#[error("squad '{id}' is already closed")]
SquadClosed { id: String },

#[error("team '{id}' already registered")]
DuplicateTeam { id: String },
```

- [ ] **Step 2: 在 `Clone` 实现中补充分支**

---

## Task 5: Squad 结果类型与构造方法

**Files:**
- Modify: `crates/tavern-comp/src/team/squad.rs`

- [ ] **Step 1: 添加 `SquadResult` 和 `Squad::new`**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SquadResult {
    pub squad_id: String,
    pub team_id: String,
    pub status: SquadStatus,
    pub context: TeamContext,
    pub outputs: Value,
}

impl Squad {
    pub fn new(
        squad_id: String,
        team_id: String,
        executor: Arc<dyn AgentExecutor>,
    ) -> Self {
        Self {
            id: squad_id.clone(),
            team_id,
            status: SquadStatus::Pending,
            context: TeamContext::default(),
            executor,
        }
    }
}
```

---

## Task 6: Mock AgentExecutor

**Files:**
- Modify: `crates/tavern-comp/src/team/executor.rs`

- [ ] **Step 1: 在文件底部添加测试模块**

```rust
#[cfg(test)]
pub mod mock {
    use super::*;
    use crate::team::role::Role;
    use futures_util::stream;
    use std::collections::HashMap;
    use std::time::Duration;

    pub struct MockAgentExecutor {
        roles: HashMap<String, Role>,
        responses: HashMap<String, Value>,
    }

    impl MockAgentExecutor {
        pub fn new(roles: Vec<Role>, responses: HashMap<String, Value>) -> Self {
            Self {
                roles: roles.into_iter().map(|r| (r.id.clone(), r)).collect(),
                responses,
            }
        }
    }

    #[async_trait]
    impl AgentExecutor for MockAgentExecutor {
        async fn resolve_role(
            &self,
            role_id: &str,
        ) -> Result<Role, AgentExecutorError> {
            self.roles
                .get(role_id)
                .cloned()
                .ok_or_else(|| AgentExecutorError::RoleNotFound { id: role_id.into() })
        }

        async fn execute(
            &self,
            role_id: &str,
            input: AgentInput,
        ) -> Result<AgentOutput, AgentExecutorError> {
            let content = self.responses.get(role_id).cloned().unwrap_or_else(|| {
                serde_json::json!({
                    "received": input.task,
                    "shared": input.context.shared,
                })
            });
            Ok(AgentOutput {
                content,
                usage: None,
                latency: Duration::from_millis(10),
                metadata: HashMap::new(),
            })
        }

        async fn execute_stream(
            &self,
            _role_id: &str,
            _input: AgentInput,
        ) -> Result<BoxStream<'static, AgentOutputChunk>, AgentExecutorError> {
            Ok(Box::pin(stream::empty()))
        }
    }
}
```

---

## Task 7: SquadEngine 核心实现

**Files:**
- Create: `crates/tavern-comp/src/team/engine.rs`

- [ ] **Step 1: 实现 `SquadEngine`**

```rust
use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;

use crate::error::CompError;
use crate::event::SquadEvent;
use crate::store::{EventStore, MemoryEventStore};
use crate::team::context::{Message, MessageKind, TeamContext};
use crate::team::definition::Team;
use crate::team::executor::{AgentExecutor, AgentInput};
use crate::team::handoff::{Handoff, HandoffMode};
use crate::team::mission::Mission;
use crate::team::squad::{Squad, SquadResult, SquadStatus};
use crate::workflow::Process;

pub struct SquadEngine {
    store: Arc<dyn EventStore>,
}

impl Default for SquadEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl SquadEngine {
    pub fn new() -> Self {
        Self {
            store: Arc::new(MemoryEventStore::new()),
        }
    }

    pub fn with_store(mut self, store: Arc<dyn EventStore>) -> Self {
        self.store = store;
        self
    }

    pub async fn deploy(
        &self,
        team: &Team,
        executor: Arc<dyn AgentExecutor>,
        inputs: Value,
    ) -> Result<Squad, CompError> {
        team.validate()?;

        let squad_id = uuid::Uuid::new_v4().to_string();
        let mut squad = Squad::new(squad_id.clone(), team.id.clone(), executor);
        squad.context.shared = inputs.clone();

        self.store
            .append(
                &squad_id,
                SquadEvent::SquadCreated {
                    squad_id: squad_id.clone(),
                    team_id: team.id.clone(),
                    inputs,
                }
                .into(),
            )
            .await?;

        Ok(squad)
    }

    pub async fn run(
        &self,
        team: &Team,
        squad: &mut Squad,
    ) -> Result<SquadResult, CompError> {
        if !matches!(team.default_process, Process::Sequential) {
            return Err(CompError::ConfigParse {
                path: "<team>".into(),
                reason: "P1 only supports sequential process".into(),
            });
        }

        squad.status = SquadStatus::Running;
        self.store
            .append(&squad.id, SquadEvent::SquadStarted.into())
            .await?;

        for mission in &team.missions {
            match self.execute_mission(squad, mission).await {
                Ok(()) => {}
                Err(e) => {
                    squad.status = SquadStatus::Failed;
                    self.store
                        .append(
                            &squad.id,
                            SquadEvent::SquadFailed {
                                reason: e.to_string(),
                                failed_at: Utc::now(),
                            }
                            .into(),
                        )
                        .await?;
                    return Err(e);
                }
            }
        }

        squad.status = SquadStatus::Completed;
        let result = SquadResult {
            squad_id: squad.id.clone(),
            team_id: squad.team_id.clone(),
            status: squad.status.clone(),
            context: squad.context.clone(),
            outputs: squad.context.shared.clone(),
        };

        self.store
            .append(
                &squad.id,
                SquadEvent::SquadCompleted {
                    outputs: result.outputs.clone(),
                    completed_at: Utc::now(),
                }
                .into(),
            )
            .await?;

        Ok(result)
    }

    async fn execute_mission(
        &self,
        squad: &mut Squad,
        mission: &Mission,
    ) -> Result<(), CompError> {
        self.store
            .append(
                &squad.id,
                SquadEvent::MissionScheduled {
                    mission_id: mission.id.clone(),
                    attempt: 1,
                }
                .into(),
            )
            .await?;

        let role = squad
            .executor
            .resolve_role(&mission.role)
            .await
            .map_err(|e| CompError::RoleNotFound {
                id: format!("{}: {}", mission.role, e),
            })?;

        let task = crate::context::render_template(&mission.task, &squad.context.shared)
            .map_err(|e| CompError::TemplateParse { reason: e.to_string() })?;

        let started_at = Utc::now();
        self.store
            .append(
                &squad.id,
                SquadEvent::MissionStarted {
                    mission_id: mission.id.clone(),
                    started_at,
                }
                .into(),
            )
            .await?;

        let input = AgentInput {
            task,
            context: squad.context.clone(),
            model_override: role.model_override.clone(),
            timeout: mission.timeout.map(std::time::Duration::from_secs),
        };

        let output = squad
            .executor
            .execute(&mission.role, input)
            .await
            .map_err(|e| CompError::StepFailed {
                step_id: mission.id.clone(),
                reason: e.to_string(),
            })?;

        let value = match mission.handoff_mode {
            HandoffMode::Required => {
                let handoff: Handoff = serde_json::from_value(output.content.clone())
                    .map_err(|e| CompError::StepFailed {
                        step_id: mission.id.clone(),
                        reason: format!("required handoff invalid: {}", e),
                    })?;
                self.record_handoff(&mut squad.context, &handoff, mission);
                handoff.payload
            }
            HandoffMode::Inherit | HandoffMode::Auto => {
                if let Some(Ok(handoff)) = Handoff::detect(&output.content) {
                    self.record_handoff(&mut squad.context, &handoff, mission);
                    handoff.payload
                } else {
                    output.content
                }
            }
        };

        if let Some(ref key) = mission.output_key {
            if let Some(obj) = squad.context.shared.as_object_mut() {
                obj.insert(key.clone(), value.clone());
            }
        }

        squad.context.thread.push(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: mission.role.clone(),
            turn: 1,
            kind: MessageKind::Output,
            content: value.clone(),
            timestamp: Utc::now(),
        });

        self.store
            .append(
                &squad.id,
                SquadEvent::MissionCompleted {
                    mission_id: mission.id.clone(),
                    output: value.clone(),
                    output_key: mission.output_key.clone(),
                    completed_at: Utc::now(),
                }
                .into(),
            )
            .await?;

        Ok(())
    }

    fn record_handoff(
        &self,
        ctx: &mut TeamContext,
        handoff: &Handoff,
        mission: &Mission,
    ) {
        ctx.thread.push(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: mission.role.clone(),
            turn: 1,
            kind: MessageKind::Handoff,
            content: serde_json::to_value(handoff).unwrap_or_default(),
            timestamp: Utc::now(),
        });
    }
}
```

---

## Task 8: TeamRegistry

**Files:**
- Create: `crates/tavern-comp/src/team/registry.rs`

- [ ] **Step 1: 实现 `TeamRegistry`**

```rust
use std::collections::HashMap;
use std::sync::RwLock;

use crate::error::CompError;
use super::definition::Team;

pub struct TeamRegistry {
    teams: RwLock<HashMap<String, Team>>,
}

impl Default for TeamRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TeamRegistry {
    pub fn new() -> Self {
        Self {
            teams: RwLock::new(HashMap::new()),
        }
    }

    pub fn register(&self, team: Team) -> Result<(), CompError> {
        team.validate()?;
        let mut teams = self.teams.write().unwrap();
        if teams.contains_key(&team.id) {
            return Err(CompError::DuplicateTeam { id: team.id.clone() });
        }
        teams.insert(team.id.clone(), team);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Team, CompError> {
        let teams = self.teams.read().unwrap();
        teams
            .get(id)
            .cloned()
            .ok_or_else(|| CompError::TeamNotFound { id: id.into() })
    }

    pub fn list(&self) -> Vec<String> {
        let teams = self.teams.read().unwrap();
        teams.keys().cloned().collect()
    }
}
```

---

## Task 9: 更新 replay.rs 处理新事件变体

**Files:**
- Modify: `crates/tavern-comp/src/replay.rs`

`replay.rs` 对 `WorkflowEvent` 做了 exhaustive match。新增 `SquadEventOccurred` 后必须更新三个地方：`event_timestamp`、`event_type_name`、`event_step_id`。`compute_state_diff` 和 summary match 暂时用 `_ => {}` 或 `_ => None` 兜底。

- [ ] **Step 1: 在 `crates/tavern-comp/src/replay.rs` 顶部添加 import**

```rust
use crate::event::SquadEvent;
```

- [ ] **Step 2: 更新 `event_timestamp`**

在 match 末尾添加：

```rust
WorkflowEvent::SquadEventOccurred { occurred_at, .. } => *occurred_at,
```

- [ ] **Step 3: 更新 `event_type_name`**

在 match 末尾添加：

```rust
WorkflowEvent::SquadEventOccurred { .. } => "SquadEventOccurred",
```

- [ ] **Step 4: 更新 `event_step_id`**

在 match 末尾添加：

```rust
WorkflowEvent::SquadEventOccurred { event, .. } => squad_event_step_id(event),
```

并新增辅助函数：

```rust
fn squad_event_step_id(event: &SquadEvent) -> Option<&str> {
    match event {
        SquadEvent::MissionScheduled { mission_id, .. }
        | SquadEvent::MissionStarted { mission_id, .. }
        | SquadEvent::MissionCompleted { mission_id, .. }
        | SquadEvent::MissionFailed { mission_id, .. } => Some(mission_id),
        _ => None,
    }
}
```

- [ ] **Step 5: 更新 `DetailLevel::includes` 中的 match**

`includes` 使用 `matches!` 宏，是 exhaustive 的。添加分支：

```rust
WorkflowEvent::SquadEventOccurred { .. }
```

到 `DetailLevel::Low` 和 `Medium` 中（ squad 事件对 workflow replay 是补充信息，medium 以上显示）。

- [ ] **Step 6: 运行 replay 测试**

```bash
cargo test -p tavern-comp --lib replay::
```

Expected: PASS

---

## Task 10: 导出更新

**Files:**
- Modify: `crates/tavern-comp/src/team/mod.rs`
- Modify: `crates/tavern-comp/src/lib.rs`

- [ ] **Step 1: `team/mod.rs`**

```rust
pub mod context;
pub mod definition;
pub mod engine;
pub mod executor;
pub mod handoff;
pub mod mission;
pub mod registry;
pub mod role;
pub mod squad;

pub use context::{Message, MessageKind, TeamContext, VisibilityRules};
pub use definition::Team;
pub use engine::SquadEngine;
pub use executor::{AgentExecutor, AgentExecutorError, AgentInput, AgentOutput, AgentOutputChunk};
pub use handoff::{AttachmentRef, AttachmentScope, Handoff, HandoffMode};
pub use mission::Mission;
pub use registry::TeamRegistry;
pub use role::{Role, SkillRef, Visibility};
pub use squad::{Squad, SquadResult, SquadStatus};
```

- [ ] **Step 2: `lib.rs`**

```rust
pub use team::{
    AgentExecutor, AgentExecutorError, AgentInput, AgentOutput, AgentOutputChunk,
    AttachmentRef, AttachmentScope, Handoff, HandoffMode, Message, MessageKind, Mission,
    Role, SkillRef, Squad, SquadEngine, SquadResult, SquadStatus, Team, TeamContext,
    TeamRegistry, Visibility, VisibilityRules,
};
```

---

## Task 11: 端到端测试

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs`

- [ ] **Step 1: 添加测试模块**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::definition::Team;
    use crate::team::executor::mock::MockAgentExecutor;
    use crate::team::handoff::HandoffMode;
    use crate::team::mission::Mission;
    use crate::team::role::{Role, Visibility};
    use crate::workflow::Process;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_team(missions: Vec<Mission>) -> Team {
        Team {
            id: "content_team".into(),
            name: "Content Team".into(),
            description: None,
            roles: vec![
                Role {
                    id: "researcher".into(),
                    name: "Researcher".into(),
                    agent_id: "base_researcher".into(),
                    visibility: Visibility::default(),
                    ..Default::default()
                },
                Role {
                    id: "writer".into(),
                    name: "Writer".into(),
                    agent_id: "base_writer".into(),
                    visibility: Visibility::default(),
                    ..Default::default()
                },
            ],
            missions,
            default_process: Process::Sequential,
            planning: None,
            webhook: None,
        }
    }

    #[tokio::test]
    async fn sequential_pipeline_runs() {
        let missions = vec![
            Mission {
                id: "research".into(),
                role: "researcher".into(),
                task: "research {{topic}}".into(),
                output_key: Some("notes".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
            Mission {
                id: "write".into(),
                role: "writer".into(),
                task: "write from {{notes}}".into(),
                depends_on: vec!["research".into()],
                output_key: Some("article".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
        ];
        let team = make_team(missions);

        let mut responses = HashMap::new();
        responses.insert("researcher".into(), serde_json::json!("research notes"));
        responses.insert("writer".into(), serde_json::json!("final article"));

        let executor = Arc::new(MockAgentExecutor::new(team.roles.clone(), responses));
        let engine = SquadEngine::new();
        let mut squad = engine
            .deploy(&team, executor, serde_json::json!({"topic": "AI"}))
            .await
            .unwrap();

        let result = engine.run(&team, &mut squad).await.unwrap();
        assert_eq!(result.outputs.get("article").unwrap(), "final article");
    }

    #[tokio::test]
    async fn sequential_pipeline_with_handoff() {
        let missions = vec![Mission {
            id: "route".into(),
            role: "researcher".into(),
            task: "decide".into(),
            output_key: Some("decision".into()),
            handoff_mode: HandoffMode::Auto,
            ..Default::default()
        }];
        let team = make_team(missions);

        let mut responses = HashMap::new();
        responses.insert(
            "researcher".into(),
            serde_json::json!({
                "summary": "go ahead",
                "payload": "approved",
                "terminate": false
            }),
        );

        let executor = Arc::new(MockAgentExecutor::new(team.roles.clone(), responses));
        let engine = SquadEngine::new();
        let mut squad = engine
            .deploy(&team, executor, serde_json::json!({}))
            .await
            .unwrap();

        let result = engine.run(&team, &mut squad).await.unwrap();
        assert_eq!(result.outputs.get("decision").unwrap(), "approved");
        assert_eq!(squad.context.thread.len(), 2); // output + handoff
    }
}
```

---

## Task 12: 最终验证

- [ ] **Step 1: 运行所有测试**

```bash
cargo test -p tavern-comp --lib
```

Expected: PASS

- [ ] **Step 2: 检查 clippy**

```bash
cargo clippy -p tavern-comp --lib
```

Expected: 无新增警告

- [ ] **Step 3: 提交**

```bash
git add crates/tavern-comp/src/team crates/tavern-comp/src/event.rs crates/tavern-comp/src/error.rs crates/tavern-comp/src/replay.rs crates/tavern-comp/src/lib.rs
git commit -m "feat(tavern): add SquadEngine for sequential Agent Team execution

- Add SquadEngine with deploy/run for sequential missions
- Add TeamRegistry for team registration/lookup
- Add SquadEvent and WorkflowEvent::SquadEventOccurred
- Update replay.rs to handle squad events
- Add MockAgentExecutor for testing
- Add end-to-end sequential pipeline tests"
```

---

## 注意事项

- P1 仅支持 `Process::Sequential`，mission 按定义顺序执行。
- Handoff 只记录到 `thread`，附件传播、next_role 调度、human-in-the-loop 留到 P2。
- `EventStore` 仍按 `WorkflowEvent` 存储；Squad 恢复需要未来在 `SquadEngine` 里重建 `TeamContext`。
- `Team::validate` 不检查 mission DAG；mission 顺序由用户/配置保证。
