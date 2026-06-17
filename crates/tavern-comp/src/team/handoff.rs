use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub enum HandoffMode {
    Inherit,
    Required,
    #[default]
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handoff {
    pub summary: String,
    #[serde(default)]
    pub next_role: Option<String>,
    #[serde(default)]
    pub candidates: Vec<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub attachments: Vec<AttachmentRef>,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub request_human: bool,
    #[serde(default)]
    pub terminate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub scope: AttachmentScope,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AttachmentScope {
    Shared,
    Private { role: String },
}

impl Handoff {
    /// Heuristic: a Value is a Handoff if it is an object with a "summary" string field.
    pub fn detect(value: &Value) -> Option<Result<Handoff, serde_json::Error>> {
        if let Some(obj) = value.as_object()
            && obj.get("summary").and_then(|v| v.as_str()).is_some()
        {
            return Some(serde_json::from_value(value.clone()));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_detection() {
        let normal = serde_json::json!("hello");
        assert!(Handoff::detect(&normal).is_none());

        let handoff = serde_json::json!({
            "summary": "done",
            "terminate": true
        });
        assert!(Handoff::detect(&handoff).is_some());
        assert!(Handoff::detect(&handoff).unwrap().unwrap().terminate);

        let not_handoff = serde_json::json!({
            "summary": 42,
            "terminate": true
        });
        assert!(Handoff::detect(&not_handoff).is_none());
    }
}
