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
        // Mission id uniqueness and dependency existence
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

        match &self.default_process {
            Process::Sequential => {
                // DAG acyclicity check via temporary Workflow
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

        Ok(())
    }

    pub fn missions(&self) -> &[Mission] {
        &self.missions
    }

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
                    model_override: None,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_yaml_deserialize() {
        let yaml = r#"
id: content_team
name: 内容生产小组
roles:
  - id: researcher
    name: 研究员
    agent_id: base_researcher
"#;
        let team: Team = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(team.id, "content_team");
        assert_eq!(team.roles.len(), 1);
        assert!(team.missions.is_empty());
    }

    #[test]
    fn team_validate_duplicate_role() {
        let team = Team {
            id: "t1".into(),
            name: "test".into(),
            description: None,
            roles: vec![
                Role {
                    id: "r1".into(),
                    name: "R1".into(),
                    agent_id: "a1".into(),
                    ..Default::default()
                },
                Role {
                    id: "r1".into(),
                    name: "R2".into(),
                    agent_id: "a2".into(),
                    ..Default::default()
                },
            ],
            missions: vec![],
            default_process: Process::Sequential,
            planning: None,
            webhook: None,
        };
        assert!(team.validate().is_err());
    }
}
