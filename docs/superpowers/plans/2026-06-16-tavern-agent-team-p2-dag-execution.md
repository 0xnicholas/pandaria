# Tavern Agent Team P2 DAG 拓扑执行计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `SquadEngine` 支持真正的 DAG 执行：missions 按 `depends_on` 拓扑排序，支持并行分支；当多个上游 mission 都完成后才执行 AND 汇合节点；`or_depends_on` 在 P2 暂不实现。

**Architecture:** 复用现有 `validator::build_dag_maps` 的算法思想，新增 `team::scheduler` 模块负责从 `Team` 构建执行计划；`SquadEngine::run` 改为调度循环：每次找出就绪 missions，使用 `tokio::sync::Semaphore` 限制并发，等待 mission 完成事件，更新上下文，继续调度直到完成。

**Tech Stack:** Rust, tokio, serde_json, std::collections

---

## 范围说明

P2 做：
- DAG 拓扑排序与就绪判断
- 并行 mission 执行（带并发控制）
- AND 依赖汇合
- 事件驱动调度循环

P2 不做：
- `or_depends_on`
- Router / Hierarchical
- `wait_for_signal` / breakpoint / retry timer
- 复杂 Handoff 调度

---

## 文件结构

- **Create:** `crates/tavern-comp/src/team/scheduler.rs` — `MissionScheduler`
- **Modify:** `crates/tavern-comp/src/team/engine.rs` — 替换顺序循环为调度循环
- **Modify:** `crates/tavern-comp/src/team/mod.rs` — 导出 scheduler（内部使用，可能不需要公开导出）
- **Modify:** `crates/tavern-comp/src/team/definition.rs` — 给 `Team::validate` 增加 mission DAG 校验
- **Modify:** `crates/tavern-comp/src/error.rs` — 已有 `MissionNotFound` 足够

---

## Task 1: Team 校验 mission DAG

**Files:**
- Modify: `crates/tavern-comp/src/team/definition.rs`

- [ ] **Step 1: 扩展 `Team::validate`**

在现有 role 校验之后，添加 mission 校验：

```rust
let mut mission_ids = std::collections::HashSet::new();
for mission in &self.missions {
    if !mission_ids.insert(mission.id.clone()) {
        return Err(CompError::ConfigParse {
            path: "<team>".into(),
            reason: format!("duplicate mission id '{}'", mission.id),
        });
    }
}

for mission in &self.missions {
    for dep in &mission.depends_on {
        if !mission_ids.contains(dep) {
            return Err(CompError::MissionNotFound { id: dep.clone() });
        }
    }
}

// DAG 无环检测
crate::validator::validate_dag(&self.to_workflow_like())?;
```

- [ ] **Step 2: 添加 `Team::to_workflow_like`**

把 `Team` 转成临时 `Workflow` 用于复用 DAG 校验：

```rust
fn to_workflow_like(&self) -> crate::workflow::Workflow {
    crate::workflow::Workflow {
        id: self.id.clone(),
        name: self.name.clone(),
        description: self.description.clone(),
        steps: self
            .missions
            .iter()
            .map(|m| crate::workflow::Step {
                id: m.id.clone(),
                agent_id: m.role.clone(),
                task: m.task.clone(),
                depends_on: m.depends_on.clone(),
                output_key: m.output_key.clone(),
                timeout: m.timeout,
                retries: m.retries,
                retry_delay: m.retry_delay,
                wait_for_signal: m.wait_for_signal.clone(),
                signal_timeout: m.signal_timeout,
                expected_output: None,
                signal_timeout_action: m.signal_timeout_action.clone(),
                breakpoint: m.breakpoint,
                model_override: m.model_override.clone(),
                or_depends_on: vec![],
                router: None,
            })
            .collect(),
        inputs: vec![],
        outputs: vec![],
        process: self.default_process.clone(),
        planning: self.planning.clone(),
        webhook: self.webhook.clone(),
        schedule: None,
        schedule_inputs: serde_json::Value::Null,
    }
}
```

- [ ] **Step 3: 运行测试**

```bash
cargo test -p tavern-comp --lib team::definition::tests
```

Expected: PASS

---

## Task 2: MissionScheduler

**Files:**
- Create: `crates/tavern-comp/src/team/scheduler.rs`

- [ ] **Step 1: 实现 `MissionScheduler`**

```rust
use std::collections::{HashMap, HashSet};

use crate::error::CompError;
use crate::team::definition::Team;
use crate::team::mission::Mission;

pub struct MissionScheduler {
    in_degree: HashMap<String, usize>,
    dependents: HashMap<String, Vec<String>>,
    missions: HashMap<String, Mission>,
}

impl MissionScheduler {
    pub fn new(team: &Team) -> Self {
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        let mut missions: HashMap<String, Mission> = HashMap::new();

        for mission in &team.missions {
            in_degree.insert(mission.id.clone(), mission.depends_on.len());
            missions.insert(mission.id.clone(), mission.clone());
        }

        for mission in &team.missions {
            for dep in &mission.depends_on {
                dependents
                    .entry(dep.clone())
                    .or_default()
                    .push(mission.id.clone());
            }
        }

        Self {
            in_degree,
            dependents,
            missions,
        }
    }

    pub fn ready(&self, completed: &HashSet<String>) -> Vec<Mission> {
        self.missions
            .values()
            .filter(|m| {
                !completed.contains(&m.id)
                    && m.depends_on.iter().all(|dep| completed.contains(dep))
            })
            .cloned()
            .collect()
    }

    pub fn all_completed(&self, completed: &HashSet<String>) -> bool {
        self.missions.keys().all(|id| completed.contains(id))
    }

    pub fn get(&self, id: &str) -> Result<Mission, CompError> {
        self.missions
            .get(id)
            .cloned()
            .ok_or_else(|| CompError::MissionNotFound { id: id.into() })
    }
}
```

- [ ] **Step 2: 添加测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::definition::Team;
    use crate::team::mission::Mission;
    use crate::team::role::Role;
    use crate::workflow::Process;

    fn make_team(missions: Vec<Mission>) -> Team {
        Team {
            id: "t1".into(),
            name: "Test".into(),
            description: None,
            roles: vec![Role {
                id: "r1".into(),
                name: "R1".into(),
                agent_id: "a1".into(),
                ..Default::default()
            }],
            missions,
            default_process: Process::Sequential,
            planning: None,
            webhook: None,
        }
    }

    #[test]
    fn scheduler_finds_ready_missions() {
        let missions = vec![
            Mission {
                id: "a".into(),
                role: "r1".into(),
                task: "do a".into(),
                ..Default::default()
            },
            Mission {
                id: "b".into(),
                role: "r1".into(),
                task: "do b".into(),
                depends_on: vec!["a".into()],
                ..Default::default()
            },
        ];
        let team = make_team(missions);
        let scheduler = MissionScheduler::new(&team);

        let completed = HashSet::new();
        let ready = scheduler.ready(&completed);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "a");

        let mut completed = HashSet::new();
        completed.insert("a".into());
        let ready = scheduler.ready(&completed);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "b");
    }

    #[test]
    fn scheduler_parallel_branches() {
        let missions = vec![
            Mission {
                id: "root".into(),
                role: "r1".into(),
                task: "root".into(),
                ..Default::default()
            },
            Mission {
                id: "left".into(),
                role: "r1".into(),
                task: "left".into(),
                depends_on: vec!["root".into()],
                ..Default::default()
            },
            Mission {
                id: "right".into(),
                role: "r1".into(),
                task: "right".into(),
                depends_on: vec!["root".into()],
                ..Default::default()
            },
            Mission {
                id: "merge".into(),
                role: "r1".into(),
                task: "merge".into(),
                depends_on: vec!["left".into(), "right".into()],
                ..Default::default()
            },
        ];
        let team = make_team(missions);
        let scheduler = MissionScheduler::new(&team);

        let mut completed = HashSet::new();
        completed.insert("root".into());
        let ready = scheduler.ready(&completed);
        let ids: Vec<String> = ready.iter().map(|m| m.id.clone()).collect();
        assert!(ids.contains(&"left".to_string()));
        assert!(ids.contains(&"right".to_string()));
        assert!(!ids.contains(&"merge".to_string()));
    }
}
```

---

## Task 3: 重构 SquadEngine 为调度循环

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs`

- [ ] **Step 1: 重构 `SquadEngine::run`**

```rust
pub async fn run(
    &self,
    team: &Team,
    squad: &mut Squad,
) -> Result<SquadResult, CompError> {
    if !matches!(team.default_process, Process::Sequential) {
        return Err(CompError::ConfigParse {
            path: "<team>".into(),
            reason: "P2 only supports sequential/dag process".into(),
        });
    }

    squad.status = SquadStatus::Running;
    self.store
        .append(&squad.id, SquadEvent::SquadStarted.into())
        .await?;

    let scheduler = MissionScheduler::new(team);
    let mut completed: HashSet<String> = HashSet::new();
    let mut running: HashSet<String> = HashSet::new();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(self.max_concurrency));

    loop {
        if scheduler.all_completed(&completed) {
            break;
        }

        let ready: Vec<Mission> = scheduler
            .ready(&completed)
            .into_iter()
            .filter(|m| !running.contains(&m.id))
            .collect();

        if ready.is_empty() && running.is_empty() {
            return Err(CompError::CyclicDependency);
        }

        if ready.is_empty() {
            // Wait for at least one running mission to complete
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            continue;
        }

        // Spawn all ready missions bounded by concurrency
        let (tx, mut rx) = tokio::sync::mpsc::channel(ready.len());
        for mission in ready {
            running.insert(mission.id.clone());
            let tx = tx.clone();
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore should not be closed");
            let engine = self.clone();
            let mut squad = squad.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let result = engine.execute_mission(&mut squad, &mission).await;
                let _ = tx.send((mission.id.clone(), result, squad)).await;
            });
        }
        drop(tx);

        // Collect results
        while let Some((mission_id, result, updated_squad)) = rx.recv().await {
            running.remove(&mission_id);
            match result {
                Ok(()) => {
                    squad.context = updated_squad.context;
                    completed.insert(mission_id);
                }
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
```

- [ ] **Step 2: 给 `SquadEngine` 添加 `max_concurrency` 字段和 `Clone`**

```rust
#[derive(Clone)]
pub struct SquadEngine {
    store: Arc<dyn EventStore>,
    max_concurrency: usize,
}

impl SquadEngine {
    pub fn new() -> Self {
        Self {
            store: Arc::new(MemoryEventStore::new()),
            max_concurrency: 4,
        }
    }

    pub fn with_max_concurrency(mut self, n: usize) -> Self {
        self.max_concurrency = n.max(1);
        self
    }
}
```

- [ ] **Step 3: `execute_mission` 不再接收 `&mut self`**

当前签名：

```rust
async fn execute_mission(
    &self,
    squad: &mut Squad,
    mission: &Mission,
) -> Result<(), CompError>
```

这个签名可以保留，因为 `execute_mission` 内部已经通过 `self.store.append` 需要 `&self`。

- [ ] **Step 4: 添加并行分支测试**

```rust
#[tokio::test]
async fn dag_parallel_branches_run() {
    let missions = vec![
        Mission {
            id: "root".into(),
            role: "researcher".into(),
            task: "root {{topic}}".into(),
            output_key: Some("root_out".into()),
            handoff_mode: HandoffMode::Inherit,
            ..Default::default()
        },
        Mission {
            id: "left".into(),
            role: "writer".into(),
            task: "left {{root_out}}".into(),
            depends_on: vec!["root".into()],
            output_key: Some("left_out".into()),
            handoff_mode: HandoffMode::Inherit,
            ..Default::default()
        },
        Mission {
            id: "right".into(),
            role: "writer".into(),
            task: "right {{root_out}}".into(),
            depends_on: vec!["root".into()],
            output_key: Some("right_out".into()),
            handoff_mode: HandoffMode::Inherit,
            ..Default::default()
        },
        Mission {
            id: "merge".into(),
            role: "editor".into(),
            task: "merge {{left_out}} {{right_out}}".into(),
            depends_on: vec!["left".into(), "right".into()],
            output_key: Some("final".into()),
            handoff_mode: HandoffMode::Inherit,
            ..Default::default()
        },
    ];

    let mut team = make_team(missions);
    team.roles.push(Role {
        id: "editor".into(),
        name: "Editor".into(),
        agent_id: "base_editor".into(),
        visibility: Visibility::default(),
        ..Default::default()
    });

    let mut responses = HashMap::new();
    responses.insert("researcher".into(), serde_json::json!("ROOT"));
    responses.insert("writer".into(), serde_json::json!("BRANCH"));

    let executor = Arc::new(MockAgentExecutor::new(team.roles.clone(), responses));
    let engine = SquadEngine::new().with_max_concurrency(2);
    let mut squad = engine
        .deploy(&team, executor, serde_json::json!({"topic": "AI"}))
        .await
        .unwrap();

    let result = engine.run(&team, &mut squad).await.unwrap();
    assert_eq!(result.outputs.get("final").unwrap(), "BRANCH BRANCH");
}
```

注意：`make_team` 当前固定只创建 researcher/writer，需要改成根据测试需要传入 roles。

---

## Task 4: 更新 make_team 辅助函数

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs`

- [ ] **Step 1: 让 `make_team` 接受可选 roles**

```rust
fn make_team(missions: Vec<Mission>, extra_roles: Option<Vec<Role>>) -> Team {
    let mut roles = vec![
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
    ];
    if let Some(extra) = extra_roles {
        roles.extend(extra);
    }

    Team {
        id: "content_team".into(),
        name: "Content Team".into(),
        description: None,
        roles,
        missions,
        default_process: Process::Sequential,
        planning: None,
        webhook: None,
    }
}
```

- [ ] **Step 2: 更新现有测试调用**

所有 `make_team(missions)` 改为 `make_team(missions, None)`。

---

## Task 5: 导出与集成

**Files:**
- Modify: `crates/tavern-comp/src/team/mod.rs`

`MissionScheduler` 是内部实现细节，不需要公开导出。`SquadEngine` 的 `with_max_concurrency` 已经是公开 API。

---

## Task 6: 最终验证

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
git add crates/tavern-comp/src/team crates/tavern-comp/src/team/scheduler.rs
git commit -m "feat(tavern): add DAG execution to SquadEngine

- Add MissionScheduler for topological ordering and ready-set computation
- Refactor SquadEngine::run into event-driven scheduling loop
- Support parallel mission execution with concurrency limit
- Add parallel branch merge test"
```

---

## 注意事项

- P2 仍不支持 `or_depends_on`，调度器只检查 `depends_on`。
- `Squad::clone` 已经实现（derive Clone），用于并发任务分割。
- 当前调度循环使用 busy-wait 轮询（10ms sleep），P3 可以改为条件变量通知。
- `execute_mission` 内部仍写 `self.store`，多个并发调用同时 append 是安全的，因为 `EventStore::append` 由具体 backend 保证。
