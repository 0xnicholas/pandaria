//! SessionStateMachine — state + error + recovery + abort_token (split from SessionActor).

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::harness::error_recovery::RecoveryStateMachine;
use crate::SessionState;

/// Encapsulates the session lifecycle state machine, error reason,
/// recovery state machine, and cancellation token. All four share the
/// same lifecycle (tied to a single SessionActor), so grouping them
/// into a single subsystem simplifies SessionActor's field surface.
pub struct SessionStateMachine {
    state: AtomicU8, // 0=Idle, 1=Running, 2=Error
    error_reason: Mutex<Option<String>>,
    recovery: RecoveryStateMachine,
    abort_token: CancellationToken,
}

impl SessionStateMachine {
    pub fn new(max_retries: u32) -> Self {
        Self {
            state: AtomicU8::new(0),
            error_reason: Mutex::new(None),
            recovery: RecoveryStateMachine::new(max_retries),
            abort_token: CancellationToken::new(),
        }
    }

    // ── State transitions ──

    pub fn enter_idle(&self) {
        self.state.store(0, Ordering::SeqCst);
    }

    pub fn enter_running(&self) {
        self.state.store(1, Ordering::SeqCst);
    }

    pub fn enter_error(&self, reason: String) {
        self.state.store(2, Ordering::SeqCst);
        *self.error_reason.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason);
    }

    pub fn clear_error(&self) {
        self.state.store(0, Ordering::SeqCst);
        self.error_reason.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    // ── Reads ──

    pub fn state(&self) -> SessionState {
        match self.state.load(Ordering::SeqCst) {
            1 => SessionState::Running,
            2 => SessionState::Error,
            _ => SessionState::Idle,
        }
    }

    pub fn is_streaming(&self) -> bool {
        self.state.load(Ordering::SeqCst) == 1
    }

    pub fn error_reason(&self) -> Option<String> {
        self.error_reason.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    #[allow(dead_code)] // Used by SessionActor (delegation arrives in Task 5)
    pub(crate) fn recovery(&self) -> &RecoveryStateMachine {
        &self.recovery
    }

    #[allow(dead_code)] // Used by SessionActor (delegation arrives in Task 5)
    pub(crate) fn recovery_mut(&mut self) -> &mut RecoveryStateMachine {
        &mut self.recovery
    }

    // ── Cancellation ──

    pub fn abort_token(&self) -> CancellationToken {
        self.abort_token.clone()
    }

    pub fn child_token(&self) -> CancellationToken {
        self.abort_token.child_token()
    }

    pub fn abort(&self) {
        self.abort_token.cancel();
    }

    pub fn reset(&mut self, max_retries: u32) {
        self.abort_token = CancellationToken::new();
        self.recovery = RecoveryStateMachine::new(max_retries);
        self.state.store(0, Ordering::SeqCst);
        *self.error_reason.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Reset only the abort token (used at the start of each run_with_messages iteration).
    #[allow(dead_code)]
    pub fn reset_abort_token(&mut self) {
        self.abort_token = CancellationToken::new();
    }

    /// Reset only the recovery state machine (used by SessionActor::reset which also clears entries).
    #[allow(dead_code)]
    pub fn reset_recovery_only(&mut self, max_retries: u32) {
        self.recovery = RecoveryStateMachine::new(max_retries);
    }

    // ── Test-only accessor (compat with test_abort_session's field access) ──

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn abort_token_ref(&self) -> &CancellationToken {
        &self.abort_token
    }
}

impl Drop for SessionStateMachine {
    fn drop(&mut self) {
        self.abort_token.cancel();
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_transitions_idle_running_idle() {
        let sm = SessionStateMachine::new(3);
        assert_eq!(sm.state(), SessionState::Idle);
        sm.enter_running();
        assert_eq!(sm.state(), SessionState::Running);
        assert!(sm.is_streaming());
        sm.enter_idle();
        assert_eq!(sm.state(), SessionState::Idle);
        assert!(!sm.is_streaming());
    }

    #[test]
    fn state_enter_error_records_reason() {
        let sm = SessionStateMachine::new(3);
        sm.enter_error("boom".to_string());
        assert_eq!(sm.state(), SessionState::Error);
        assert_eq!(sm.error_reason(), Some("boom".to_string()));
    }

    #[test]
    fn state_clear_error_resets_to_idle() {
        let sm = SessionStateMachine::new(3);
        sm.enter_error("oops".to_string());
        sm.clear_error();
        assert_eq!(sm.state(), SessionState::Idle);
        assert_eq!(sm.error_reason(), None);
    }

    #[test]
    fn state_abort_propagates_to_child_token() {
        let sm = SessionStateMachine::new(3);
        let child = sm.child_token();
        assert!(!child.is_cancelled());
        sm.abort();
        assert!(child.is_cancelled());
        assert!(sm.abort_token().is_cancelled());
    }

    #[test]
    fn state_reset_provides_fresh_token() {
        let mut sm = SessionStateMachine::new(3);
        let old_token = sm.abort_token();
        sm.abort();
        assert!(old_token.is_cancelled());
        sm.reset(5);
        assert!(!sm.abort_token().is_cancelled());
        assert_eq!(sm.recovery().max_attempts(), 5);
    }
}