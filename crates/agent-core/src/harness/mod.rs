pub mod agent_loop;
pub mod compaction;
pub mod error_recovery;
pub mod session;
pub mod tool;

pub use agent_loop::*;
pub use compaction::*;
pub use session::{SessionActor, SessionConfig};
// ToolExecutor is pub(crate) — not re-exported at harness boundary.
