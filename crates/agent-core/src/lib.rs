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
pub mod hook_dispatcher;
pub mod mutations;
pub mod session;
pub mod store;
pub mod tool;
pub mod types;

#[path = "loop.rs"]
pub mod loop_;

pub use compaction::*;
pub use context::*;
pub use error::AgentError;
pub use hook_dispatcher::HookDispatcher;
pub use loop_::AgentLoop;
pub use mutations::*;
pub use session::SessionActor;
pub use store::SessionStore;
pub use tool::ToolExecutor;
pub use types::*;
