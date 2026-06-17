use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageKind {
    Invocation,
    Output,
    Handoff,
    Observation,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: String,
    pub turn: u32,
    pub kind: MessageKind,
    pub content: Value,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VisibilityRules {
    /// role id -> list of other role private spaces it can read
    #[serde(default)]
    pub role_can_read: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TeamContext {
    #[serde(default)]
    pub shared: Value,
    #[serde(default)]
    pub private: HashMap<String, Value>,
    #[serde(default)]
    pub thread: Vec<Message>,
    #[serde(default)]
    pub visibility: VisibilityRules,
}

impl TeamContext {
    pub fn can_read_private(&self, reader_role: &str, owner_role: &str) -> bool {
        reader_role == owner_role
            || self
                .visibility
                .role_can_read
                .get(reader_role)
                .map(|v| v.contains(&owner_role.to_string()))
                .unwrap_or(false)
    }

    /// P0: minimal resolution. Full priority (shared -> own private -> authorized private ->
    /// _last_message) will be implemented in template rendering integration.
    pub fn resolve(&self, role: &str, key: &str) -> Option<&Value> {
        self.shared.get(key).or_else(|| {
            if self.can_read_private(role, role) {
                self.private.get(role).and_then(|v| v.get(key))
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_private_visibility() {
        let mut ctx = TeamContext::default();
        ctx.private.insert("a".into(), serde_json::json!({"x": 1}));
        assert!(ctx.can_read_private("a", "a"));
        assert!(!ctx.can_read_private("b", "a"));
    }

    #[test]
    fn context_resolve_own_private() {
        let mut ctx = TeamContext::default();
        ctx.shared = serde_json::json!({"shared_key": "shared_value"});
        ctx.private.insert(
            "a".into(),
            serde_json::json!({"private_key": "private_value"}),
        );
        assert_eq!(
            ctx.resolve("a", "shared_key"),
            Some(&serde_json::json!("shared_value"))
        );
        assert_eq!(
            ctx.resolve("a", "private_key"),
            Some(&serde_json::json!("private_value"))
        );
        assert_eq!(ctx.resolve("b", "private_key"), None);
    }
}
