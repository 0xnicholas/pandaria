use std::collections::{HashMap, HashSet};

use crate::error::CompError;
use crate::team::definition::Team;
use crate::team::mission::Mission;

/// Mission 调度器：根据 `depends_on` 计算就绪集合。
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
