//! Agent-core: the core agent loop runtime, session lifecycle, and
//! dependency-inversion boundaries for hooks and persistence.
//!
//! # Architecture
//!
//! - [`AgentLoop`] drives the tool-use protocol per ADR-001
//! - [`SessionActor`] manages per-tenant session lifecycle
//! - [`HookDispatcher`] is the extension boundary (ADR-002/ADR-003)
//! - [`SessionStore`] is the persistence boundary (ADR-005)
//! - [`ToolExecutor`] implements the tool execution pipeline

pub mod compaction;
pub mod context;
pub mod error;
pub mod error_recovery;
pub mod events;
pub mod file_ops;
pub mod hook_dispatcher;
pub mod mutations;
pub mod provider_opts;
pub mod session;
pub mod session_entry;
pub mod store;
pub mod tool;
pub mod types;

pub(crate) mod hook_timeout;
pub(crate) mod util;

#[path = "loop.rs"]
pub mod loop_;

pub mod test_utils;

pub use compaction::*;
pub use context::*;
pub use error::{AgentError, CompactionError};
pub use error_recovery::{RecoveryAction, RecoveryStateMachine};
pub use events::{AgentEvent, AgentEventListener};
pub use file_ops::{DefaultFileOperationExtractor, FileOperationExtractor, FileOperations};
pub use hook_dispatcher::HookDispatcher;
pub use loop_::{AgentLoop, AgentLoopConfig, TurnResult};
pub use mutations::*;
pub use provider_opts::ProviderStreamOptions;
pub use session::SessionActor;
pub use session_entry::{CompactionDetails, SessionContextBuilder, SessionEntry};
pub use store::SessionStore;
pub use tool::ToolExecutor;
pub use types::*;
