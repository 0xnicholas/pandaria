use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::events::SessionEventBridge;
use crate::manager::SessionInfo;
use crate::supervisor::SessionGuard;

/// Handle to an active session managed by `TenantManagerImpl`.
pub struct ActiveSession {
    pub actor: Arc<Mutex<agent_core::SessionActor>>,
    pub abort_token: CancellationToken,
    pub _guard: SessionGuard,
    pub info: SessionInfo,
    pub bridge: Arc<SessionEventBridge>,
    pub turn_counter: AtomicU64,
}
