pub mod agent_loop;
pub mod builder;
pub mod compaction;
pub mod config;
pub mod error_recovery;
pub mod session;
pub mod strategy;
pub mod tool;

pub use agent_loop::*;
pub use builder::{BuiltSession, SessionBuilder};
pub use compaction::*;
pub use config::{HarnessConfig, HookConfig};
pub use session::{SessionActor, SessionConfig, SessionState};
pub use strategy::{
    ContextStrategy, CriteriaEvaluation, GoalCriterion, GoalExhaustedAction, GoalOutcome,
    GoalVerification, RhythmStrategy, SessionStrategy, TerminationStrategy,
    DEFAULT_LOOP_INTERVAL,
};
// ToolExecutor is pub(crate) — not re-exported at harness boundary.
