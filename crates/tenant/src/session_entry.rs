use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::events::SessionEventBridge;
use crate::manager::SessionInfo;
use crate::supervisor::SessionGuard;

/// Handle to an active session managed by `TenantManagerImpl`.
pub struct ActiveSession {
    pub actor: Arc<Mutex<agent_core::SessionActor>>,
    /// Mutable abort token shared with `SessionActor`.
    /// `SessionActor::reset()` updates its internal token; `reset_session()`
    /// propagates the new token here so that subsequent `interrupt()` calls
    /// target the correct cancellation handle.
    pub abort_token: Arc<std::sync::Mutex<CancellationToken>>,
    pub _guard: SessionGuard,
    pub info: SessionInfo,
    pub bridge: Arc<SessionEventBridge>,
    pub turn_counter: AtomicU64,
    /// Tools registered for this session (used by clone_session).
    pub tools: Vec<agent_core::AgentToolRef>,
    /// Webhook configuration for this session.
    pub webhook: Option<crate::manager::WebhookConfig>,
    /// Original tool configs as supplied by the caller (used by clone_session
    /// so that `MediaGenerationTool` and other built-ins are re-injected
    /// rather than lossily serialised back to `ToolConfig`).
    pub original_tools: Vec<agent_core::ToolConfig>,
    /// Pawbun builtin tools config for clone propagation.
    pub builtin_tools_enabled: bool,
    pub builtin_tools_disabled: Vec<String>,
}
