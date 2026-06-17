use std::collections::{HashMap, HashSet};

use crate::error::CompError;
use crate::team::definition::Team;
use crate::team::mission::Mission;

/// Mission 调度器：根据 `depends_on`（AND 依赖）和 `or_depends_on`（OR 依赖）计算就绪集合。
///
/// 就绪条件：
/// - 所有 `depends_on` 均已完成（AND 语义）
/// - 至少一个 `or_depends_on` 已完成，若 `or_depends_on` 为空则跳过（OR 语义）
pub struct MissionScheduler {
    missions: HashMap<String, Mission>,
}

impl MissionScheduler {
    pub fn new(team: &Team) -> Self {
        let mut missions: HashMap<String, Mission> = HashMap::new();
        for mission in &team.missions {
            missions.insert(mission.id.clone(), mission.clone());
        }
        Self { missions }
    }

    pub fn ready(&self, completed: &HashSet<String>) -> Vec<Mission> {
        self.missions
            .values()
            .filter(|m| {
                if completed.contains(&m.id) {
                    return false;
                }
                // AND dependencies: all must be completed
                let and_ok = m.depends_on.iter().all(|dep| completed.contains(dep));
                // OR dependencies: at least one must be completed (if any specified)
                let or_ok = m.or_depends_on.is_empty()
                    || m.or_depends_on.iter().any(|dep| completed.contains(dep));
                and_ok && or_ok
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

    #[test]
    fn or_depends_on_any_completed_triggers() {
        let missions = vec![
            Mission {
                id: "fast".into(),
                role: "r1".into(),
                task: "fast research".into(),
                ..Default::default()
            },
            Mission {
                id: "deep".into(),
                role: "r1".into(),
                task: "deep research".into(),
                ..Default::default()
            },
            Mission {
                id: "write".into(),
                role: "r1".into(),
                task: "write article".into(),
                or_depends_on: vec!["fast".into(), "deep".into()],
                ..Default::default()
            },
        ];
        let team = make_team(missions);
        let scheduler = MissionScheduler::new(&team);

        // Only fast completes — write should be ready (OR satisfied)
        let mut completed = HashSet::new();
        completed.insert("fast".into());
        let ready = scheduler.ready(&completed);
        let ids: Vec<String> = ready.iter().map(|m| m.id.clone()).collect();
        assert!(ids.contains(&"deep".to_string())); // deep has no deps, always ready
        assert!(ids.contains(&"write".to_string())); // OR satisfied by fast
    }

    #[test]
    fn or_depends_on_none_completed_blocks() {
        let missions = vec![
            Mission {
                id: "a".into(),
                role: "r1".into(),
                task: "task a".into(),
                ..Default::default()
            },
            Mission {
                id: "b".into(),
                role: "r1".into(),
                task: "task b".into(),
                or_depends_on: vec!["a".into()],
                ..Default::default()
            },
        ];
        let team = make_team(missions);
        let scheduler = MissionScheduler::new(&team);

        // Nothing completed — b should NOT be ready (OR not satisfied)
        let completed = HashSet::new();
        let ready = scheduler.ready(&completed);
        let ids: Vec<String> = ready.iter().map(|m| m.id.clone()).collect();
        assert!(ids.contains(&"a".to_string()));
        assert!(!ids.contains(&"b".to_string()));
    }

    #[test]
    fn mixed_and_or_dependencies() {
        let missions = vec![
            Mission {
                id: "mandatory".into(),
                role: "r1".into(),
                task: "must complete".into(),
                ..Default::default()
            },
            Mission {
                id: "optional_a".into(),
                role: "r1".into(),
                task: "optional a".into(),
                ..Default::default()
            },
            Mission {
                id: "optional_b".into(),
                role: "r1".into(),
                task: "optional b".into(),
                ..Default::default()
            },
            Mission {
                id: "final".into(),
                role: "r1".into(),
                task: "final task".into(),
                depends_on: vec!["mandatory".into()],        // AND
                or_depends_on: vec!["optional_a".into(), "optional_b".into()], // OR
                ..Default::default()
            },
        ];
        let team = make_team(missions);
        let scheduler = MissionScheduler::new(&team);

        // mandatory + optional_a done → final should be ready
        let mut completed = HashSet::new();
        completed.insert("mandatory".into());
        completed.insert("optional_a".into());
        let ready = scheduler.ready(&completed);
        let ids: Vec<String> = ready.iter().map(|m| m.id.clone()).collect();
        assert!(ids.contains(&"optional_b".to_string()));
        assert!(ids.contains(&"final".to_string()));

        // Only mandatory done, neither optional → final should NOT be ready
        let mut completed = HashSet::new();
        completed.insert("mandatory".into());
        let ready = scheduler.ready(&completed);
        let ids: Vec<String> = ready.iter().map(|m| m.id.clone()).collect();
        assert!(!ids.contains(&"final".to_string()));
    }
}
