# Tavern Agent Team P3 Hierarchical Manager-Worker 计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `SquadEngine` 支持 `Process::Hierarchical`：Manager role 动态决定下一步执行哪个 mission，直到所有 mission 完成或 Manager 输出 done。

**Architecture:** 扩展 `SquadEngine::run` 支持两种模式：DAG 模式（P2）和 Hierarchical 模式。Hierarchical 模式下，循环询问 Manager role，解析其返回的 `Handoff` 决定 `next_role` / 具体 mission；执行该 mission 后把结果反馈给 Manager，继续下一轮。设置 `MAX_MANAGER_LOOPS` 防止无限循环。

**Tech Stack:** Rust, tokio, serde_json, tavern-core

---

## 范围说明

P3 做：
- `Team` 支持 `Process::Hierarchical(ManagerConfig)`
- `SquadEngine` Hierarchical 执行循环
- Manager prompt 构建
- Manager 返回解析（JSON / Handoff）
- 最大 loop 保护
- 基础单元测试

P3 不做：
- Planning 阶段集成（已有 `Team.planning`，但 Hierarchical 内不调用）
- 复杂 human-in-the-loop
- OR 依赖 / Router

---

## 文件结构

- **Modify:** `crates/tavern-comp/src/team/definition.rs` — `Team::validate` 支持 Hierarchical
- **Modify:** `crates/tavern-comp/src/team/engine.rs` — 添加 `run_hierarchical` 和 manager prompt
- **Modify:** `crates/tavern-comp/src/team/scheduler.rs` — 添加按 id 查找和全部 mission id 列表
- **Modify:** `crates/tavern-comp/src/error.rs` — 添加 `ManagerLoopExceeded`（已存在，复用即可）

---

## Task 1: Team 校验支持 Hierarchical

**Files:**
- Modify: `crates/tavern-comp/src/team/definition.rs`

- [ ] **Step 1: 修改 `Team::validate`**

移除 "P2 only supports sequential" 限制，改为：

```rust
match &self.default_process {
    Process::Sequential => {
        // DAG 校验
        crate::validator::validate_dag(&self.to_workflow_like())?;
    }
    Process::Hierarchical(cfg) => {
        // manager 的 role id 必须在 team.roles 中存在
        if !self.roles.iter().any(|r| r.id == cfg.agent_id) {
            return Err(CompError::ConfigParse {
                path: "<team>".into(),
                reason: format!(
                    "hierarchical manager role '{}' not found in team roles",
                    cfg.agent_id
                ),
            });
        }
    }
}
```

- [ ] **Step 2: 运行测试**

```bash
cargo test -p tavern-comp --lib team::definition::tests
```

Expected: PASS

---

## Task 2: （保留 Scheduler 不变）

`MissionScheduler` 已有 `ready` 和 `all_completed`，Hierarchical 模式足够使用。

---

## Task 3: Hierarchical 执行循环

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs`

- [ ] **Step 1: 修改 `SquadEngine::run` 分发**

```rust
pub async fn run(
    &self,
    team: &Team,
    squad: &mut Squad,
) -> Result<SquadResult, CompError> {
    squad.status = SquadStatus::Running;
    self.store
        .append(&squad.id, SquadEvent::SquadStarted.into())
        .await?;

    match &team.default_process {
        Process::Sequential => self.run_dag(team, squad).await,
        Process::Hierarchical(cfg) => self.run_hierarchical(team, squad, cfg).await,
    }
}
```

- [ ] **Step 2: 重命名原 run 逻辑为 `run_dag`**

把原 `run` 函数体提取为 `async fn run_dag(&self, team: &Team, squad: &mut Squad) -> Result<SquadResult, CompError>`。

- [ ] **Step 3: 实现 `run_hierarchical`**

```rust
const MAX_MANAGER_LOOPS: usize = 100;

async fn run_hierarchical(
    &self,
    team: &Team,
    squad: &mut Squad,
    manager_cfg: &tavern_core::ManagerConfig,
) -> Result<SquadResult, CompError> {
    let scheduler = MissionScheduler::new(team);
    let mut completed: HashSet<String> = HashSet::new();
    let mut manager_loops: usize = 0;

    loop {
        if scheduler.all_completed(&completed) {
            break;
        }

        manager_loops += 1;
        if manager_loops > MAX_MANAGER_LOOPS {
            squad.status = SquadStatus::Failed;
            self.store
                .append(
                    &squad.id,
                    SquadEvent::SquadFailed {
                        reason: format!("manager loop exceeded {} iterations", MAX_MANAGER_LOOPS),
                        failed_at: Utc::now(),
                    }
                    .into(),
                )
                .await?;
            return Err(CompError::ManagerLoopExceeded {
                max_loops: MAX_MANAGER_LOOPS,
            });
        }

        let prompt = self.build_manager_prompt(team, squad, &completed, manager_cfg);
        let input = AgentInput {
            task: prompt,
            context: squad.context.clone(),
            model_override: None,
            timeout: Some(std::time::Duration::from_secs(60)),
        };

        let output = squad
            .executor
            .execute(&manager_cfg.agent_id, input)
            .await
            .map_err(|e| CompError::ManagerError {
                reason: format!("manager execution failed: {}", e),
            })?;

        let handoff: Handoff = match Handoff::detect(&output.content) {
            Some(Ok(h)) => h,
            Some(Err(e)) => {
                return Err(CompError::ManagerError {
                    reason: format!("manager output is not valid Handoff: {}", e),
                })
            }
            None => {
                // Try plain JSON with action field for backward compatibility
                serde_json::from_value::<Handoff>(output.content.clone())
                    .map_err(|e| CompError::ManagerError {
                        reason: format!("manager output must be a Handoff object: {}", e),
                    })?
            }
        };

        if handoff.terminate {
            break;
        }

        let next_mission = if let Some(ref role_id) = handoff.next_role {
            team.missions
                .iter()
                .find(|m| m.role == *role_id && !completed.contains(&m.id))
                .cloned()
                .ok_or_else(|| CompError::ManagerError {
                    reason: format!(
                        "manager requested role '{}' but no pending mission found",
                        role_id
                    ),
                })?
        } else if !handoff.candidates.is_empty() {
            let role_id = &handoff.candidates[0];
            team.missions
                .iter()
                .find(|m| m.role == *role_id && !completed.contains(&m.id))
                .cloned()
                .ok_or_else(|| CompError::ManagerError {
                    reason: format!(
                        "manager requested candidate role '{}' but no pending mission found",
                        role_id
                    ),
                })?
        } else {
            return Err(CompError::ManagerError {
                reason: "manager returned handoff without next_role or candidates".into(),
            });
        };

        // Execute the delegated mission
        let mut branch_squad = squad.clone();
        match self.execute_mission(&mut branch_squad, &next_mission).await {
            Ok(()) => {
                merge_context(&mut squad.context, &branch_squad.context);
                completed.insert(next_mission.id.clone());
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

- [ ] **Step 4: 实现 manager prompt**

```rust
fn build_manager_prompt(
    &self,
    team: &Team,
    squad: &Squad,
    completed: &HashSet<String>,
    manager_cfg: &tavern_core::ManagerConfig,
) -> String {
    let system = manager_cfg
        .instructions
        .as_deref()
        .unwrap_or("You are a project manager. Decide the next mission to delegate.");

    let roles_desc = team
        .roles
        .iter()
        .map(|r| {
            format!(
                "- {} (agent_id: {}): {}",
                r.id,
                r.agent_id,
                r.description.as_deref().unwrap_or("no description")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let pending_desc = team
        .missions
        .iter()
        .filter(|m| !completed.contains(&m.id))
        .map(|m| format!("- {} [role: {}]: {}", m.id, m.role, m.task))
        .collect::<Vec<_>>()
        .join("\n");

    let completed_desc = team
        .missions
        .iter()
        .filter(|m| completed.contains(&m.id))
        .map(|m| {
            let output = squad
                .context
                .shared
                .get(&m.output_key.clone().unwrap_or_default())
                .cloned()
                .unwrap_or_default();
            format!("- {} [role: {}]: {} -> {}", m.id, m.role, m.task, output)
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "{}\n\n## Available Roles\n{}\n\n## Pending Missions\n{}\n\n## Completed Missions\n{}\n\n## Context\n{}\n\n## Output Format\nRespond with a JSON object matching this Handoff schema:\n{{\n  \"summary\": \"why you chose this\",\n  \"next_role\": \"role_id_of_next_mission\",\n  \"instructions\": \"optional extra instructions\"\n}}\nTo finish, set \"terminate\": true.",
        system,
        roles_desc,
        pending_desc,
        completed_desc,
        squad.context.shared
    )
}
```

---

## Task 4: 测试

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs`

- [ ] **Step 1: 添加 Hierarchical 测试**

```rust
#[tokio::test]
async fn hierarchical_manager_delegates() {
    use tavern_core::ManagerConfig;
    use crate::team::executor::stateful_mock::StatefulMockExecutor;

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
            output_key: Some("article".into()),
            handoff_mode: HandoffMode::Inherit,
            ..Default::default()
        },
    ];

    let team = Team {
        id: "content_team".into(),
        name: "Content Team".into(),
        description: None,
        roles: vec![
            Role {
                id: "manager".into(),
                name: "Manager".into(),
                agent_id: "base_manager".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
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
        default_process: Process::Hierarchical(ManagerConfig {
            agent_id: "manager".into(),
            instructions: None,
        }),
        planning: None,
        webhook: None,
    };

    let mut sequences = HashMap::new();
    sequences.insert(
        "manager".into(),
        vec![
            serde_json::json!({"summary": "start with research", "next_role": "researcher"}),
            serde_json::json!({"summary": "now write", "next_role": "writer"}),
            serde_json::json!({"summary": "done", "terminate": true}),
        ],
    );
    sequences.insert("researcher".into(), vec![serde_json::json!("research notes")]);
    sequences.insert("writer".into(), vec![serde_json::json!("final article")]);

    let executor = Arc::new(StatefulMockExecutor::new(
        team.roles.clone(),
        sequences,
        serde_json::Value::Null,
    ));
    let engine = SquadEngine::new();
    let mut squad = engine
        .deploy(&team, executor, serde_json::json!({"topic": "AI"}))
        .await
        .unwrap();

    let result = engine.run(&team, &mut squad).await.unwrap();
    assert_eq!(result.outputs.get("article").unwrap(), "final article");
}
```


---

## Task 5: 实现 StatefulMockExecutor 用于测试

**Files:**
- Modify: `crates/tavern-comp/src/team/executor.rs`

- [ ] **Step 1: 添加 `StatefulMockExecutor`**

```rust
#[cfg(test)]
pub mod stateful_mock {
    use super::*;
    use crate::team::role::Role;
    use futures_util::stream;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Mock executor that returns a sequence of responses for a given role.
    pub struct StatefulMockExecutor {
        roles: HashMap<String, Role>,
        sequences: Mutex<HashMap<String, Vec<Value>>>,
        default: Value,
    }

    impl StatefulMockExecutor {
        pub fn new(
            roles: Vec<Role>,
            sequences: HashMap<String, Vec<Value>>,
            default: Value,
        ) -> Self {
            Self {
                roles: roles.into_iter().map(|r| (r.id.clone(), r)).collect(),
                sequences: Mutex::new(sequences),
                default,
            }
        }
    }

    #[async_trait]
    impl AgentExecutor for StatefulMockExecutor {
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
            let content = {
                let mut seq = self.sequences.lock().unwrap();
                if let Some(items) = seq.get_mut(role_id) {
                    if !items.is_empty() {
                        items.remove(0)
                    } else {
                        self.default.clone()
                    }
                } else {
                    self.default.clone()
                }
            };

            let content = if content == Value::Null {
                serde_json::json!({
                    "received": input.task,
                    "shared": input.context.shared,
                })
            } else {
                content
            };

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
git commit -m "feat(tavern): add Hierarchical Manager-Worker mode to SquadEngine

- Support Process::Hierarchical with dynamic manager delegation
- Add manager prompt builder and Handoff response parser
- Add max manager loop protection
- Add StatefulMockExecutor for testing dynamic responses"
```

---

## 注意事项

- P3 中 Manager 返回的 `next_role` 用于选择该 role 的下一个未完成的 mission。
- 如果多个 mission 共享同一个 role，Manager 无法指定具体 mission id，只能指定 role。P4 可以扩展 `Handoff` 支持 `next_mission_id`。
- Hierarchical 模式下不检查 DAG，由 Manager 决定执行顺序。
- `terminate: true` 会立即结束 squad，即使还有未完成的 missions。
