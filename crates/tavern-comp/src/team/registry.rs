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
