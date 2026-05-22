pub mod agent_loop;
pub mod builder;
pub mod compaction;
pub mod config;
pub mod error_recovery;
pub mod session;
pub mod tool;

pub use agent_loop::*;
pub use builder::{BuiltSession, SessionBuilder};
pub use compaction::*;
pub use config::{HarnessConfig, HookConfig};
pub use session::{SessionActor, SessionConfig, SessionState};
// ToolExecutor is pub(crate) — not re-exported at harness boundary.
