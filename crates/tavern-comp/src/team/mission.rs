use serde::{Deserialize, Serialize};

use crate::workflow::SignalTimeoutAction;
use super::handoff::HandoffMode;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mission {
    pub id: String,
    pub role: String,
    pub task: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub or_depends_on: Vec<String>,
    #[serde(default)]
    pub output_key: Option<String>,
    #[serde(default)]
    pub handoff_mode: HandoffMode,

    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub retries: Option<u64>,
    #[serde(default)]
    pub retry_delay: Option<u64>,
    #[serde(default)]
    pub wait_for_signal: Option<String>,
    #[serde(default)]
    pub signal_timeout: Option<u64>,
    #[serde(default)]
    pub signal_timeout_action: Option<SignalTimeoutAction>,
    #[serde(default)]
    pub breakpoint: bool,
}
