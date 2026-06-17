use std::collections::HashSet;
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
use crate::team::scheduler::MissionScheduler;
use crate::team::squad::{Squad, SquadResult, SquadStatus};
use crate::workflow::Process;

/// Timeout for the planning agent (seconds).
const PLANNING_TIMEOUT_SECS: u64 = 120;

#[derive(Clone)]
pub struct SquadEngine {
    store: Arc<dyn EventStore>,
    max_concurrency: usize,
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
            max_concurrency: 4,
        }
    }

    pub fn with_store(mut self, store: Arc<dyn EventStore>) -> Self {
        self.store = store;
        self
    }

    pub fn with_max_concurrency(mut self, n: usize) -> Self {
        self.max_concurrency = n.max(1);
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
        squad.status = SquadStatus::Running;
        self.store
            .append(&squad.id, SquadEvent::SquadStarted.into())
            .await?;

        // Planning phase: if enabled, call planner agent to analyze missions
        // and inject plan context before execution.
        let effective_team = if let Some(ref planning) = team.planning
            && planning.enabled
        {
            self.run_planning_phase(team, squad).await?
        } else {
            team.clone()
        };

        let result = match &effective_team.default_process {
            Process::Sequential => self.run_dag(&effective_team, squad).await,
            Process::Hierarchical(cfg) => self.run_hierarchical(&effective_team, squad, cfg).await,
        };

        // Flush executor state regardless of outcome (best-effort, log on error)
        if let Err(e) = squad.executor.flush().await {
            tracing::warn!(
                squad_id = %squad.id,
                error = %e,
                "squad executor flush failed"
            );
        }

        result
    }

    /// Planning phase: invoke the planner agent to analyze missions and produce
    /// a structured plan. The plan's strategy and per-mission reasoning are
    /// injected into mission tasks as context. In Sequential mode, the planner
    /// may override `depends_on` to establish execution order.
    async fn run_planning_phase(
        &self,
        team: &Team,
        squad: &Squad,
    ) -> Result<Team, CompError> {
        let planning = team.planning.as_ref().unwrap();
        let planner_role_id = planning
            .planning_agent
            .as_deref()
            .unwrap_or(&team.roles[0].id);

        let planner_prompt = build_planner_prompt(team, &squad.context.shared);

        let input = AgentInput {
            task: planner_prompt,
            context: squad.context.clone(),
            model_override: None,
            timeout: Some(std::time::Duration::from_secs(PLANNING_TIMEOUT_SECS)),
            squad_id: Some(squad.id.clone()),
            mission_id: None,
        };

        let output = squad
            .executor
            .execute(planner_role_id, input)
            .await
            .map_err(|e| CompError::PlanningError {
                reason: format!("planner agent execution failed: {}", e),
            })?;

        // Extract text from output (handles both String and Object values)
        let response_str = match &output.content {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };

        let plan: tavern_core::Plan =
            parse_json_with_retry(&response_str).map_err(|e| CompError::PlanningError {
                reason: format!("failed to parse plan JSON: {}", e),
            })?;

        // Validate plan references
        let mission_ids: std::collections::HashSet<&str> =
            team.missions.iter().map(|m| m.id.as_str()).collect();
        for ps in &plan.steps {
            if !mission_ids.contains(ps.task_id.as_str()) {
                return Err(CompError::PlanningError {
                    reason: format!("plan references unknown mission_id: {}", ps.task_id),
                });
            }
        }

        // Inject plan context into missions
        let mut planned_team = team.clone();
        for mission in &mut planned_team.missions {
            if let Some(plan_step) = plan.steps.iter().find(|ps| ps.task_id == mission.id) {
                let plan_context = format!(
                    "\n\n[Plan Context]\nOverall Strategy: {}\nYour role in this plan: {}\nExpected output: {}",
                    plan.overall_strategy, plan_step.reasoning, plan_step.expected_output
                );
                mission.task = format!("{}{}", mission.task, plan_context);

                // In Sequential mode, override depends_on with planner's suggested deps
                if matches!(team.default_process, Process::Sequential)
                    && !plan_step.dependencies.is_empty()
                {
                    mission.depends_on = plan_step.dependencies.clone();
                }
            }
        }

        // Re-validate DAG after planner modified dependencies
        planned_team.validate()?;

        Ok(planned_team)
    }

    /// P2: DAG-compatible sequential execution with parallel branches.
    async fn run_dag(
        &self,
        team: &Team,
        squad: &mut Squad,
    ) -> Result<SquadResult, CompError> {
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
                        // Merge outputs: shared context and thread from the completed branch.
                        // P2 constraint: parallel missions should write to disjoint output keys.
                        merge_context(&mut squad.context, &updated_squad.context);
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

    /// P3: Hierarchical Manager-Worker execution.
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
            if manager_loops > Self::MAX_MANAGER_LOOPS {
                squad.status = SquadStatus::Failed;
                self.store
                    .append(
                        &squad.id,
                        SquadEvent::SquadFailed {
                            reason: format!(
                                "manager loop exceeded {} iterations",
                                Self::MAX_MANAGER_LOOPS
                            ),
                            failed_at: Utc::now(),
                        }
                        .into(),
                    )
                    .await?;
                return Err(CompError::ManagerLoopExceeded {
                    max_loops: Self::MAX_MANAGER_LOOPS,
                });
            }

            let prompt = self.build_manager_prompt(team, squad, &completed, manager_cfg);
            let input = AgentInput {
                task: prompt,
                context: squad.context.clone(),
                model_override: None,
                timeout: Some(std::time::Duration::from_secs(60)),
                squad_id: Some(squad.id.clone()),
                mission_id: None,
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
                None => serde_json::from_value::<Handoff>(output.content.clone()).map_err(
                    |e| CompError::ManagerError {
                        reason: format!("manager output must be a Handoff object: {}", e),
                    },
                )?,
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
                    .get(m.output_key.clone().unwrap_or_default())
                    .cloned()
                    .unwrap_or_default();
                format!("- {} [role: {}]: {} -> {}", m.id, m.role, m.task, output)
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "{}\n\n## Available Roles\n{}\n\n## Pending Missions\n{}\n\n## Completed Missions\n{}\n\n## Context\n{}\n\n## Output Format\nRespond with a JSON object matching this Handoff schema:\n{{\n  \"summary\": \"why you chose this\",\n  \"next_role\": \"role_id_of_next_mission\",\n  \"instructions\": \"optional extra instructions\",\n  \"terminate\": false\n}}\nTo finish, set \"terminate\": true.",
            system,
            roles_desc,
            pending_desc,
            completed_desc,
            squad.context.shared
        )
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
            squad_id: Some(squad.id.clone()),
            mission_id: Some(mission.id.clone()),
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

        if let Some(ref key) = mission.output_key
            && let Some(obj) = squad.context.shared.as_object_mut()
        {
            obj.insert(key.clone(), value.clone());
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

// ── Planning helpers ───────────────────────────────────────────────────────

/// Build the prompt for the planning agent.
fn build_planner_prompt(team: &Team, shared: &Value) -> String {
    let mut missions_desc = String::new();
    for mission in &team.missions {
        missions_desc.push_str(&format!(
            "- id: {}\n  role: {}\n  task: {}\n",
            mission.id, mission.role, mission.task
        ));
        if !mission.depends_on.is_empty() {
            missions_desc.push_str(&format!("  depends_on: {:?}\n", mission.depends_on));
        }
        if let Some(ref key) = mission.output_key {
            missions_desc.push_str(&format!("  output_key: {}\n", key));
        }
    }

    let roles_desc: String = team
        .roles
        .iter()
        .map(|r| {
            format!(
                "- {} (agent: {}): {}",
                r.id,
                r.agent_id,
                r.description.as_deref().unwrap_or("no description")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "You are a planning agent for team: {}\n\n\
         Available Roles:\n{}\n\n\
         Missions to plan:\n{}\n\n\
         Shared Context:\n{}\n\n\
         Output a JSON plan with:\n\
         - overall_strategy: string\n\
         - steps: [\n\
             {{\"task_id\": \"...\", \"agent_id\": \"...\", \"reasoning\": \"...\", \n\
               \"expected_output\": \"...\", \"dependencies\": [\"...\"]}}\n\
           ]",
        team.description.as_deref().unwrap_or(&team.name),
        roles_desc,
        missions_desc,
        shared,
    )
}

/// Extract JSON from LLM output that may contain markdown fences or extra text.
fn extract_json(raw: &str) -> String {
    if serde_json::from_str::<Value>(raw).is_ok() {
        return raw.to_string();
    }
    if let Some(start) = raw.find("```json") {
        let after = &raw[start + 7..];
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
    }
    if let Some(start) = raw.find("```") {
        let after = &raw[start + 3..];
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
    }
    if let Some(start) = raw.find('{')
        && let Some(end) = raw.rfind('}')
    {
        return raw[start..=end].to_string();
    }
    raw.to_string()
}

/// Parse JSON with one retry attempt (used for planning output).
fn parse_json_with_retry<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, String> {
    let json_str = extract_json(raw);
    serde_json::from_str(&json_str).map_err(|e| format!("invalid JSON: {}", e))
}

/// Merge outputs from a parallel branch back into the main squad context.
/// P2 constraint: parallel missions must write to disjoint output keys to avoid races.
fn merge_context(main: &mut TeamContext, branch: &TeamContext) {
    // Merge shared object keys
    if let (Some(main_obj), Some(branch_obj)) =
        (main.shared.as_object_mut(), branch.shared.as_object())
    {
        for (key, value) in branch_obj {
            main_obj.insert(key.clone(), value.clone());
        }
    }

    // Append branch messages to the main thread
    main.thread.extend(branch.thread.iter().cloned());

    // Merge private spaces (overwrites with branch values; P2 does not define cross-role policies)
    for (role, value) in &branch.private {
        main.private.insert(role.clone(), value.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::definition::Team;
    use crate::team::executor::mock::MockAgentExecutor;
    use crate::team::executor::stateful_mock::StatefulMockExecutor;
    use crate::team::handoff::HandoffMode;
    use crate::team::mission::Mission;
    use crate::team::role::{Role, Visibility};
    use crate::workflow::Process;
    use std::collections::HashMap;

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
        let team = make_team(missions, None);

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
        let team = make_team(missions, None);

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
        assert!(squad.context.thread.len() >= 2); // output + handoff
    }

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

        let team = make_team(
            missions,
            Some(vec![Role {
                id: "editor".into(),
                name: "Editor".into(),
                agent_id: "base_editor".into(),
                visibility: Visibility::default(),
                ..Default::default()
            }]),
        );

        let mut responses = HashMap::new();
        responses.insert("researcher".into(), serde_json::json!("ROOT"));
        responses.insert("writer".into(), serde_json::json!("BRANCH"));
        responses.insert("editor".into(), serde_json::json!("BRANCH BRANCH"));

        let executor = Arc::new(MockAgentExecutor::new(team.roles.clone(), responses));
        let engine = SquadEngine::new().with_max_concurrency(2);
        let mut squad = engine
            .deploy(&team, executor, serde_json::json!({"topic": "AI"}))
            .await
            .unwrap();

        let result = engine.run(&team, &mut squad).await.unwrap();
        assert_eq!(result.outputs.get("final").unwrap(), "BRANCH BRANCH");
    }

    #[tokio::test]
    async fn dag_detects_cycle() {
        let missions = vec![
            Mission {
                id: "a".into(),
                role: "writer".into(),
                task: "a".into(),
                depends_on: vec!["b".into()],
                ..Default::default()
            },
            Mission {
                id: "b".into(),
                role: "writer".into(),
                task: "b".into(),
                depends_on: vec!["a".into()],
                ..Default::default()
            },
        ];
        let team = make_team(missions, None);
        assert!(team.validate().is_err());
    }

    #[tokio::test]
    async fn hierarchical_manager_delegates() {
        use tavern_core::ManagerConfig;

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
}
