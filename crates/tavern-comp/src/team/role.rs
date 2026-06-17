use serde::{Deserialize, Serialize};
use tavern_core::{ModelConfig, SkillConfig};

/// Agent Team 内对 skill 的引用。P0 直接复用 `SkillConfig`。
pub type SkillRef = SkillConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Role {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub agent_id: String,
    #[serde(default)]
    pub team_instructions: Option<String>,
    #[serde(default)]
    pub model_override: Option<ModelConfig>,
    #[serde(default)]
    pub visibility: Visibility,
    #[serde(default)]
    pub skills: Vec<SkillRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Visibility {
    #[serde(default = "default_true")]
    pub read_shared: bool,
    #[serde(default)]
    pub read_private_roles: Vec<String>,
}

impl Default for Visibility {
    fn default() -> Self {
        Self {
            read_shared: true,
            read_private_roles: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_defaults() {
        let role = Role {
            id: "researcher".into(),
            name: "研究员".into(),
            description: None,
            agent_id: "base_researcher".into(),
            team_instructions: None,
            model_override: None,
            visibility: Visibility::default(),
            skills: vec![],
        };
        assert!(role.visibility.read_shared);
        assert!(role.visibility.read_private_roles.is_empty());
    }

    #[test]
    fn visibility_deserialize_defaults() {
        let yaml = "read_private_roles: [other]";
        let v: Visibility = serde_yaml::from_str(yaml).unwrap();
        assert!(v.read_shared);
        assert_eq!(v.read_private_roles, vec!["other".to_string()]);
    }
}
