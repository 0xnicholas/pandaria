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

// Core runtime (harness)
pub mod harness;

// Hook protocol and extension boundary
pub mod hook;

// Persistence boundary
pub mod persistence;

// Utilities
pub mod utils;

// Top-level modules
pub mod circuit_breaker;
pub mod error;
pub mod events;
pub mod file_ops;
pub mod memory;
pub mod prompt;
pub mod runtime;
pub mod skills;
pub mod tools;
#[cfg(any(test, feature = "testing"))]
pub mod test_utils;
pub mod space;
pub mod types;

// ═══ Compatibility re-exports ═══
// These keep existing `use agent_core::SessionActor` paths working.

pub use harness::{
    compaction,
    compaction::{CompactionActor, CompactionConfig, CompactionPreparation, CompactionResult},
    session::{SessionActor, SessionConfig, SessionState},
};

pub use runtime::{RuntimeConfig, SessionBuilder, BuiltSession, DefaultHookConfig};

pub use harness::agent_loop::{AgentLoop, AgentLoopConfig, TurnResult};

pub use hook::{
    context,
    default_dispatcher::DefaultHookDispatcher,
    dispatcher::HookDispatcher,
    mutations,
    timeout::with_timeout,
};

pub use persistence::{
    entry::{CompactionDetails, SessionContextBuilder, SessionEntry},
    store::SessionStore,
};

pub use utils::provider_opts::ProviderStreamOptions;

pub use prompt::{PromptBuilder, PromptMutation};

pub use skills::{
    FileSystemSkillLoader, LoadSkillsResult, Skill, SkillDiagnostic, SkillDiagnosticKind,
    SkillFrontmatter, SkillLoader, SkillSource, format_skills_for_prompt, parse_skill_invocation,
};

pub use error::{AgentError, CompactionError};
pub use events::{AgentEvent, AgentEventListener};
pub use file_ops::{DefaultFileOperationExtractor, FileOperationExtractor, FileOperations};
pub use space::AgentSpace;
pub use tools::{HttpProxyTool, MediaGenerationTool, ToolConfig};
pub use types::*;

// Re-export ai-provider types used in public API
pub use ai_provider::{Content, ToolResultMessage};
