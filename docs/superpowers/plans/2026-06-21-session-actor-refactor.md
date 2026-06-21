# SessionActor 拆分重构 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `crates/agent-core/src/harness/session.rs`（2625 行）拆分为 `SessionHistory` + `SessionEventHub` + `SessionStateMachine` + 瘦 `SessionActor`，公开 API 100% 兼容。

**Architecture:** 把原 SessionActor 的 29 个字段按职责分到 3 个值类型子结构（无 Arc 共享，session 独占），SessionActor 保留 17 个顶层字段 + 3 个子系统。`#[cfg(any(test, feature = "testing"))] pub(crate)` accessor 兼容 2 处私有字段测试访问。3 个 `pub(crate)` getter（`event_tx_clone` / `steer_queue_clone` / `follow_up_queue_clone`）让 SessionActor 构造 `AgentLoopConfig`。

**Tech Stack:** Rust 2024 edition、tokio、async-trait、thiserror、tokio_util::sync::CancellationToken。无需新增依赖。

**Spec:** `docs/superpowers/specs/2026-06-21-session-actor-refactor-design.md`

**Pre-flight ground truth (verified from codebase):**
- `ai-provider` crate exports `Message` enum (not `AgentMessage`); see `crates/ai-provider/src/types.rs:217`.
- `AgentMessage` is a type alias defined at `crates/agent-core/src/types.rs:2`: `pub type AgentMessage = ai_provider::Message;`.
- `struct QueuedEvent` is defined at `crates/agent-core/src/harness/session.rs:27` (will be moved to event_hub.rs in Task 4).
- `truncate_entries_before(first_kept_entry_id)` is the existing method (line 1097), called from `run_auto_compaction` (970) and `apply_context_strategy_before_run` (1088).

---

## 文件结构总览

| 文件 | 操作 | 行数目标 | 职责 |
|---|---|---|---|
| `crates/agent-core/src/harness/session.rs` | 删除 | — | 旧的单文件 SessionActor |
| `crates/agent-core/src/harness/session/mod.rs` | 新建 | ≤ 800 | SessionActor 瘦 orchestrator + SessionConfig + re-export |
| `crates/agent-core/src/harness/session/history.rs` | 新建 | ≤ 300 | SessionHistory（entries + 持久化 + restore + flush） |
| `crates/agent-core/src/harness/session/event_hub.rs` | 新建 | ≤ 250 | SessionEventHub（事件 + steer/follow_up + processor） |
| `crates/agent-core/src/harness/session/state.rs` | 新建 | ≤ 200 | SessionStateMachine（state + error + recovery + abort_token） |
| `crates/agent-core/src/harness/error_recovery.rs` | 改 1 处 | — | 给 `RecoveryStateMachine` 加 `pub fn max_attempts(&self) -> u32` |
| `crates/agent-core/src/harness/mod.rs` | 不变 | — | `pub mod session;` 已是 |
| `crates/agent-core/src/lib.rs` | 不变 | — | `pub use harness::session::{...}` 保留 |

---

## Phase 1：新建文件骨架

### Task 1: 创建 `session/` 目录骨架

**Files:**
- Rename: `crates/agent-core/src/harness/session.rs` → `crates/agent-core/src/harness/session/old.rs`
- Create: `crates/agent-core/src/harness/session/mod.rs`（临时引用 old.rs）
- Create: `crates/agent-core/src/harness/session/{history,event_hub,state}.rs`（空骨架）

- [ ] **Step 1: 重命名 session.rs 为 session/old.rs**

```bash
mkdir -p crates/agent-core/src/harness/session
git mv crates/agent-core/src/harness/session.rs crates/agent-core/src/harness/session/old.rs
```

- [ ] **Step 2: 创建 session/mod.rs 临时引用 old.rs**

```bash
cat > crates/agent-core/src/harness/session/mod.rs <<'EOF'
// Temporary re-export of old SessionActor during refactor.
// Will be replaced by the slim SessionActor + 3 subsystems in Phase 2/3.
pub use old::*;
EOF
```

- [ ] **Step 3: 创建 3 个空骨架**

```bash
cat > crates/agent-core/src/harness/session/history.rs <<'EOF'
// SessionHistory — owns message entries + persistence.
// Implementation arrives in Task 3.
EOF

cat > crates/agent-core/src/harness/session/event_hub.rs <<'EOF'
// SessionEventHub — owns event system + steer/follow-up queues.
// Implementation arrives in Task 4.
EOF

cat > crates/agent-core/src/harness/session/state.rs <<'EOF'
// SessionStateMachine — owns state + error + recovery + abort_token.
// Implementation arrives in Task 2.
EOF
```

- [ ] **Step 4: 编译验证**

Run:
```bash
cargo build -p agent-core 2>&1 | tail -20
```

Expected: `Finished` 无 error。

- [ ] **Step 5: Commit**

```bash
git add crates/agent-core/src/harness/session/
git commit -m "refactor(session): scaffold session/ directory with old.rs as fallback"
```

---

## Phase 2：子系统实现

### Task 2: 实现 SessionStateMachine

**Files:**
- Modify: `crates/agent-core/src/harness/error_recovery.rs`（加 1 个 getter）
- Modify: `crates/agent-core/src/harness/session/state.rs`（完整实现）
- Modify: `crates/agent-core/src/harness/session/mod.rs`（加 re-export）

- [ ] **Step 1: 给 RecoveryStateMachine 加 max_attempts getter**

在 `crates/agent-core/src/harness/error_recovery.rs` 中找到 `impl RecoveryStateMachine` 块，加：

```rust
impl RecoveryStateMachine {
    // ... existing methods ...

    /// Maximum retry attempts configured for this recovery loop.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }
}
```

如果 `max_attempts` 字段名不是 `max_attempts`，grep `self\.max_attempts\|self\.limit` 找出实际字段名。

- [ ] **Step 2: 跑现有测试验证 getter 不破坏**

```bash
cargo test -p agent-core --lib harness::error_recovery 2>&1 | tail -10
```

Expected: 通过。

- [ ] **Step 3: 写 SessionStateMachine 失败测试**

写到 `crates/agent-core/src/harness/session/state.rs` 末尾（先于实现，让其编译失败）：

```rust
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
        let sm = SessionStateMachine::new(3);
        let old_token = sm.abort_token();
        sm.abort();
        assert!(old_token.is_cancelled());
        sm.reset(5);
        assert!(!sm.abort_token().is_cancelled());
        assert_eq!(sm.recovery().max_attempts(), 5);
    }
}
```

- [ ] **Step 4: 跑测试确认失败**

Run:
```bash
cargo test -p agent-core --lib harness::session::state::tests 2>&1 | tail -10
```

Expected: `cannot find type SessionStateMachine`。

- [ ] **Step 5: 写 SessionStateMachine 完整实现**

替换 `crates/agent-core/src/harness/session/state.rs` 内容：

```rust
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

    pub fn recovery(&self) -> &RecoveryStateMachine {
        &self.recovery
    }

    pub fn recovery_mut(&mut self) -> &mut RecoveryStateMachine {
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
        let sm = SessionStateMachine::new(3);
        let old_token = sm.abort_token();
        sm.abort();
        assert!(old_token.is_cancelled());
        sm.reset(5);
        assert!(!sm.abort_token().is_cancelled());
        assert_eq!(sm.recovery().max_attempts(), 5);
    }
}
```

- [ ] **Step 6: 在 mod.rs 暴露 state 模块**

修改 `crates/agent-core/src/harness/session/mod.rs`：

```rust
pub mod state;

pub use state::SessionStateMachine;

// Temporary re-export of old SessionActor during refactor.
pub use old::*;
```

- [ ] **Step 7: 跑测试验证通过**

Run:
```bash
cargo test -p agent-core --lib harness::session::state::tests 2>&1 | tail -10
```

Expected: 5 tests passed。

- [ ] **Step 8: 编译验证整体 crate**

```bash
cargo build -p agent-core 2>&1 | tail -10
```

Expected: `Finished`。

- [ ] **Step 9: Commit**

```bash
git add crates/agent-core/src/harness/error_recovery.rs \
        crates/agent-core/src/harness/session/state.rs \
        crates/agent-core/src/harness/session/mod.rs
git commit -m "feat(session): add SessionStateMachine subsystem with 5 unit tests"
```

---

### Task 3: 实现 SessionHistory

**Files:**
- Modify: `crates/agent-core/src/harness/session/history.rs`（完整实现）
- Modify: `crates/agent-core/src/harness/session/mod.rs`（加 re-export）

- [ ] **Step 1: 写 SessionHistory 失败测试（Step 2 是实现）**

注意：`AgentMessage` 是 `crate::types::AgentMessage`（在 agent-core 内部 type alias，不是 ai-provider crate 直接导出的）。

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::entry::SessionEntry;
    use crate::types::AgentMessage;
    use ai_provider::{AssistantMessage, Content, StopReason, Usage};
    use std::time::SystemTime;

    fn msg(text: &str) -> AgentMessage {
        AgentMessage::Assistant(AssistantMessage {
            content: vec![Content::Text { text: text.to_string(), text_signature: None }],
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            error_message: None,
            timestamp: SystemTime::now(),
        })
    }

    #[tokio::test]
    async fn history_push_and_messages() {
        let mut h = SessionHistory::new("t1", "s1", None);
        assert!(h.is_empty());
        h.push(msg("hello"));
        h.push(msg("world"));
        assert_eq!(h.len(), 2);
        assert_eq!(h.messages().len(), 2);
    }

    #[tokio::test]
    async fn history_auto_restore_empty_store() {
        let mut h = SessionHistory::new("t1", "s1", None);
        h.auto_restore().await.unwrap();
        assert!(h.is_empty());
    }

    #[tokio::test]
    async fn history_auto_restore_resets_needs_restore_on_success() {
        // With a real store returning empty entries, needs_restore flag is consumed.
        // We test via the public behavior: after auto_restore() returns Ok, subsequent
        // calls are no-ops (don't re-fetch).
        let mut h = SessionHistory::new("t1", "s1", None);
        h.auto_restore().await.unwrap();
        // Second call should be idempotent — no panic, no double-fetch.
        h.auto_restore().await.unwrap();
        assert!(h.is_empty());
    }

    #[tokio::test]
    async fn history_estimate_tokens_via_compactor() {
        let mut h = SessionHistory::new("t1", "s1", None);
        h.push(msg("hello world this is a test"));
        assert!(h.estimate_tokens() > 0);
    }

    #[test]
    fn history_truncate_before_removes_strictly_older_entries() {
        let mut h = SessionHistory::new("t1", "s1", None);
        h.push(msg("a"));
        h.push(msg("b"));
        // Capture the ID of "b" (the boundary entry to keep).
        let b_id = match h.entries().last().unwrap() {
            SessionEntry::Message { id, .. } => *id,
            _ => panic!("expected Message entry"),
        };
        h.push(msg("c"));
        // After truncate_before(b_id), entries strictly before "b" are removed.
        // "a" is removed; "b" and "c" remain.
        h.truncate_before(b_id);
        assert_eq!(h.len(), 2, "should keep 'b' and 'c'");
    }

    #[test]
    fn history_truncate_before_handles_missing_boundary_gracefully() {
        let mut h = SessionHistory::new("t1", "s1", None);
        h.push(msg("a"));
        // Nonexistent boundary is a no-op.
        h.truncate_before(uuid::Uuid::new_v4());
        assert_eq!(h.len(), 1);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run:
```bash
cargo test -p agent-core --lib harness::session::history::tests 2>&1 | tail -10
```

Expected: `cannot find type SessionHistory`。

- [ ] **Step 3: 写 SessionHistory 完整实现**

```rust
//! SessionHistory — owns message entries + persistence + restore + flush.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;

use crate::error::AgentError;
use crate::harness::compaction::estimate_context_tokens;
use crate::persistence::entry::SessionEntry;
use crate::persistence::store::SessionStore;
use crate::types::AgentMessage;

pub struct SessionHistory {
    tenant_id: String,
    session_id: String,
    entries: Vec<SessionEntry>,
    store: Option<Arc<dyn SessionStore>>,
    needs_restore: bool,
    last_saved_entry_count: usize,
    last_save: Option<JoinHandle<()>>,
}

impl SessionHistory {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        store: Option<Arc<dyn SessionStore>>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            entries: Vec::new(),
            store,
            needs_restore: true,
            last_saved_entry_count: 0,
            last_save: None,
        }
    }

    // ── Message operations ──

    pub fn push(&mut self, msg: AgentMessage) {
        self.entries.push(SessionEntry::Message {
            id: uuid::Uuid::new_v4(),
            message: msg,
        });
    }

    pub fn append_compaction(&mut self, entry: SessionEntry) {
        self.entries.push(entry);
    }

    /// Remove entries strictly before the boundary (entries[boundary] is kept).
    /// No-op if the boundary ID is not found.
    pub fn truncate_before(&mut self, boundary: uuid::Uuid) {
        if let Some(idx) = self.entries.iter().position(|e| e.id() == boundary) {
            self.entries.drain(..idx);
        }
    }

    // ── Reads ──

    pub fn messages(&self) -> Vec<AgentMessage> {
        self.entries.iter().filter_map(|e| match e {
            SessionEntry::Message { message, .. } => Some(message.clone()),
            SessionEntry::Compaction { .. } => None,
        }).collect()
    }

    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn last_compaction_timestamp(&self) -> Option<SystemTime> {
        self.entries.iter().rev().find_map(|e| match e {
            SessionEntry::Compaction { timestamp, .. } => Some(*timestamp),
            SessionEntry::Message { .. } => None,
        })
    }

    // ── Persistence ──

    /// First call: load from store. Subsequent calls: no-op.
    /// On load failure, keeps `needs_restore = false` (avoid retry loop) but logs.
    pub async fn auto_restore(&mut self) -> Result<(), AgentError> {
        if !self.needs_restore {
            return Ok(());
        }
        self.needs_restore = false;
        if let Some(ref store) = self.store {
            match store.load_session(&self.tenant_id, &self.session_id).await {
                Ok(entries) if !entries.is_empty() => {
                    let count = entries.len();
                    self.entries = entries;
                    self.last_saved_entry_count = count;
                    tracing::info!(
                        tenant_id = %self.tenant_id,
                        session_id = %self.session_id,
                        restored_count = count,
                        "auto-restored session history",
                    );
                }
                Ok(_) => {} // empty store, fresh session
                Err(e) => {
                    tracing::warn!(
                        tenant_id = %self.tenant_id,
                        session_id = %self.session_id,
                        error = %e,
                        "auto-restore failed, starting with empty session",
                    );
                }
            }
        }
        Ok(())
    }

    /// Save only newly added entries since the last save boundary.
    /// Awaits the previous in-flight save (with 5s timeout) to preserve ordering,
    /// then spawns a new fire-and-forget save task.
    pub async fn persist_incremental(&mut self) {
        let Some(ref store) = self.store else { return; };
        if let Some(handle) = self.last_save.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }
        let new_entries = &self.entries[self.last_saved_entry_count..];
        if new_entries.is_empty() {
            return;
        }
        let entries_to_save = new_entries.to_vec();
        self.last_saved_entry_count = self.entries.len();
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();
        let store = store.clone();
        self.last_save = Some(tokio::spawn(async move {
            if let Err(e) = store.append_entries(&tenant_id, &session_id, &entries_to_save).await {
                tracing::warn!(
                    tenant_id = %tenant_id,
                    session_id = %session_id,
                    error = %e,
                    "failed to persist session",
                );
            }
        }));
    }

    pub fn persist_status(&self, status: &str) {
        let Some(ref store) = self.store else { return; };
        let store = store.clone();
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();
        let status = status.to_string();
        tokio::spawn(async move {
            if let Err(e) = store.update_session_status(&tenant_id, &session_id, &status).await {
                tracing::warn!(
                    %tenant_id,
                    %session_id,
                    %status,
                    error = %e,
                    "failed to persist session status",
                );
            }
        });
    }

    pub async fn flush(&mut self) -> Result<(), AgentError> {
        if let Some(handle) = self.last_save.take() {
            let _ = handle.await;
        }
        if let Some(ref store) = self.store {
            store.save_session(&self.tenant_id, &self.session_id, &self.entries).await?;
            tracing::info!(
                tenant_id = %self.tenant_id,
                session_id = %self.session_id,
                "session state flushed to store",
            );
        }
        Ok(())
    }

    // ── Test-only accessor (compat with test_entries_api_with_compaction's field access) ──

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn entries_mut(&mut self) -> &mut Vec<SessionEntry> {
        &mut self.entries
    }

    // ── Capacity ──

    pub fn estimate_tokens(&self) -> usize {
        estimate_context_tokens(&self.entries)
    }
}
```

- [ ] **Step 4: 在 mod.rs 暴露 history 模块**

```rust
pub mod history;
pub mod state;

pub use history::SessionHistory;
pub use state::SessionStateMachine;

pub use old::*;
```

- [ ] **Step 5: 跑测试验证通过**

Run:
```bash
cargo test -p agent-core --lib harness::session::history::tests 2>&1 | tail -10
```

Expected: 6 tests passed。

- [ ] **Step 6: 编译验证整体 crate**

```bash
cargo build -p agent-core 2>&1 | tail -10
```

Expected: `Finished`。

- [ ] **Step 7: Commit**

```bash
git add crates/agent-core/src/harness/session/history.rs \
        crates/agent-core/src/harness/session/mod.rs
git commit -m "feat(session): add SessionHistory subsystem with 6 unit tests"
```

---

### Task 4: 实现 SessionEventHub

**Files:**
- Modify: `crates/agent-core/src/harness/session/event_hub.rs`（完整实现）
- Modify: `crates/agent-core/src/harness/session/mod.rs`（加 re-export）

- [ ] **Step 1: 写 SessionEventHub 失败测试**

注意：`AgentMessage::default()` 不存在，必须构造具体 UserMessage。

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentEvent, AgentEventListener};
    use crate::types::AgentMessage;
    use ai_provider::{Content, UserMessage};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    struct CountingListener(Arc<AtomicUsize>);
    #[async_trait]
    impl AgentEventListener for CountingListener {
        async fn on_event(&self, _: &AgentEvent) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: vec![Content::Text { text: text.to_string(), text_signature: None }],
            timestamp: SystemTime::now(),
        })
    }

    #[tokio::test]
    async fn event_hub_listener_receives_events() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut hub = SessionEventHub::new();
        hub.add_listener(Arc::new(CountingListener(counter.clone())));
        hub.emit(AgentEvent::StateChanged { state: crate::SessionState::Idle });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn event_hub_steer_drain() {
        let hub = SessionEventHub::new();
        hub.steer(user_msg("s1"));
        hub.steer(user_msg("s2"));
        assert_eq!(hub.drain_steer().len(), 2);
        assert!(hub.drain_steer().is_empty(), "second drain should be empty");
    }

    #[tokio::test]
    async fn event_hub_follow_up_drain() {
        let hub = SessionEventHub::new();
        hub.follow_up(user_msg("f1"));
        assert_eq!(hub.drain_follow_up().len(), 1);
        assert!(hub.drain_follow_up().is_empty());
    }

    #[tokio::test]
    async fn event_hub_shutdown_terminates_processor() {
        let mut hub = SessionEventHub::new();
        hub.shutdown().await;
        // After shutdown, emit should not panic (sender dropped).
        hub.emit(AgentEvent::StateChanged { state: crate::SessionState::Idle });
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run:
```bash
cargo test -p agent-core --lib harness::session::event_hub::tests 2>&1 | tail -10
```

Expected: `cannot find type SessionEventHub`。

- [ ] **Step 3: 写 SessionEventHub 完整实现**

```rust
//! SessionEventHub — owns event system + steer/follow-up queues + processor.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::events::{AgentEvent, AgentEventListener};
use crate::types::AgentMessage;

/// Event queue wrapper (pub(crate) — only SessionEventHub and SessionActor's event_sink closure use it).
pub(crate) struct QueuedEvent {
    pub event: AgentEvent,
}

pub struct SessionEventHub {
    listeners: Arc<Mutex<Vec<Arc<dyn AgentEventListener>>>>,
    event_tx: Option<mpsc::Sender<QueuedEvent>>,
    event_processor_handle: Option<JoinHandle<()>>,
    steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
}

impl SessionEventHub {
    pub fn new() -> Self {
        let (tx, mut rx) = mpsc::channel::<QueuedEvent>(1024);
        let listeners: Arc<Mutex<Vec<Arc<dyn AgentEventListener>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let listeners_for_task = listeners.clone();
        let handle = tokio::spawn(async move {
            while let Some(queued) = rx.recv().await {
                let ls: Vec<_> = listeners_for_task
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .cloned()
                    .collect();
                for listener in &ls {
                    let _ = listener.on_event(&queued.event).await;
                }
            }
        });
        Self {
            listeners,
            event_tx: Some(tx),
            event_processor_handle: Some(handle),
            steer_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
        }
    }

    // ── Events ──

    pub fn emit(&self, event: AgentEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(QueuedEvent { event });
        }
    }

    pub fn add_listener(&mut self, listener: Arc<dyn AgentEventListener>) {
        self.listeners.lock().unwrap_or_else(|e| e.into_inner()).push(listener);
    }

    // ── Steer / Follow-up ──

    pub fn steer(&self, msg: AgentMessage) {
        self.steer_queue.lock().expect("steer queue poisoned").push(msg);
    }

    pub fn follow_up(&self, msg: AgentMessage) {
        self.follow_up_queue.lock().expect("follow_up queue poisoned").push(msg);
    }

    pub fn drain_steer(&self) -> Vec<AgentMessage> {
        std::mem::take(&mut *self.steer_queue.lock().expect("steer queue poisoned"))
    }

    pub fn drain_follow_up(&self) -> Vec<AgentMessage> {
        std::mem::take(&mut *self.follow_up_queue.lock().expect("follow_up queue poisoned"))
    }

    // ── Internal accessors (pub(crate) — SessionActor uses these to build AgentLoopConfig) ──

    pub(crate) fn event_tx_clone(&self) -> Option<mpsc::Sender<QueuedEvent>> {
        self.event_tx.clone()
    }

    pub(crate) fn steer_queue_clone(&self) -> Arc<Mutex<Vec<AgentMessage>>> {
        self.steer_queue.clone()
    }

    pub(crate) fn follow_up_queue_clone(&self) -> Arc<Mutex<Vec<AgentMessage>>> {
        self.follow_up_queue.clone()
    }

    // ── Lifecycle ──

    pub async fn shutdown(&mut self) {
        // Drop sender so the processor sees channel closed.
        self.event_tx.take();
        if let Some(handle) = self.event_processor_handle.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), handle).await;
        }
    }
}

impl Default for SessionEventHub {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SessionEventHub {
    fn drop(&mut self) {
        self.event_tx.take();
        let _ = self.event_processor_handle.take();
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::AgentEvent;
    use crate::types::AgentMessage;
    use ai_provider::{Content, UserMessage};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    struct CountingListener(Arc<AtomicUsize>);
    #[async_trait]
    impl AgentEventListener for CountingListener {
        async fn on_event(&self, _: &AgentEvent) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: vec![Content::Text { text: text.to_string(), text_signature: None }],
            timestamp: SystemTime::now(),
        })
    }

    #[tokio::test]
    async fn event_hub_listener_receives_events() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut hub = SessionEventHub::new();
        hub.add_listener(Arc::new(CountingListener(counter.clone())));
        hub.emit(AgentEvent::StateChanged { state: crate::SessionState::Idle });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn event_hub_steer_drain() {
        let hub = SessionEventHub::new();
        hub.steer(user_msg("s1"));
        hub.steer(user_msg("s2"));
        assert_eq!(hub.drain_steer().len(), 2);
        assert!(hub.drain_steer().is_empty(), "second drain should be empty");
    }

    #[tokio::test]
    async fn event_hub_follow_up_drain() {
        let hub = SessionEventHub::new();
        hub.follow_up(user_msg("f1"));
        assert_eq!(hub.drain_follow_up().len(), 1);
        assert!(hub.drain_follow_up().is_empty());
    }

    #[tokio::test]
    async fn event_hub_shutdown_terminates_processor() {
        let mut hub = SessionEventHub::new();
        hub.shutdown().await;
        hub.emit(AgentEvent::StateChanged { state: crate::SessionState::Idle });
    }
}
```

- [ ] **Step 4: 在 mod.rs 暴露 event_hub 模块**

```rust
pub mod event_hub;
pub mod history;
pub mod state;

pub use event_hub::SessionEventHub;
pub use history::SessionHistory;
pub use state::SessionStateMachine;

pub use old::*;
```

- [ ] **Step 5: 跑测试验证通过**

Run:
```bash
cargo test -p agent-core --lib harness::session::event_hub::tests 2>&1 | tail -10
```

Expected: 4 tests passed。

- [ ] **Step 6: 编译验证**

```bash
cargo build -p agent-core 2>&1 | tail -10
```

Expected: `Finished`。

- [ ] **Step 7: Commit**

```bash
git add crates/agent-core/src/harness/session/event_hub.rs \
        crates/agent-core/src/harness/session/mod.rs
git commit -m "feat(session): add SessionEventHub subsystem with 4 unit tests"
```

---

## Phase 3：SessionActor 委托化 + 删除字段

### Task 5: SessionActor 委托化（保留字段，方法改为委托）

**Files:**
- Modify: `crates/agent-core/src/harness/session/old.rs`

- [ ] **Step 1: 在 SessionActor 加 3 个子系统字段**

在 `old.rs` 的 `SessionActor` struct 定义中，加：

```rust
    history: SessionHistory,
    event_hub: SessionEventHub,
    state_machine: SessionStateMachine,
```

- [ ] **Step 2: 初始化子系统字段**

在 `SessionActor::new()` 末尾加：

```rust
        let state_machine = SessionStateMachine::new(3);
        let history = SessionHistory::new(
            config.tenant_id.clone(),
            config.session_id.clone(),
            config.store.clone(),
        );
        let event_hub = SessionEventHub::new();
```

然后在 `actor = Self { ... }` 的 struct 初始化块末尾加：

```rust
            history,
            event_hub,
            state_machine,
```

- [ ] **Step 3: 把 SessionActor 方法改为委托（完整表）**

按以下完整表逐个替换方法体（保留 `#[instrument]` 等属性、签名）：

#### Public API

| 方法 | 新实现 |
|---|---|
| `push_message(msg)` | `self.history.push(msg)` |
| `steer(msg)` | `self.event_hub.steer(msg)` |
| `follow_up(msg)` | `self.event_hub.follow_up(msg)` |
| `messages()` | `self.history.messages()` |
| `entries()` | `self.history.entries()` |
| `flush()` | `async { self.history.flush().await }` |
| `state()` | `self.state_machine.state()` |
| `is_streaming()` | `self.state_machine.is_streaming()` |
| `error_reason()` | `self.state_machine.error_reason()` |
| `abort_token()` | `self.state_machine.abort_token()` |
| `abort()` | `self.state_machine.abort()` |
| `reset()` | `self.state_machine.reset(self.max_retries)` |
| `shutdown()` | 序列：`self.state_machine.abort()` → `self.history.flush().await` → `self.event_hub.shutdown().await` |
| `add_event_listener(l)` | `self.event_hub.add_listener(l)` |

#### Internal helpers（私有方法）

| 方法 | 新实现 |
|---|---|
| `emit_event(e)` (fn) | `self.event_hub.emit(e)` |
| `persist_status(s)` (fn) | `self.history.persist_status(s)` |
| `truncate_entries_before(uuid)` (fn, **rename → `truncate_before`**) | `self.history.truncate_before(uuid)` |
| `apply_context_strategy_before_run` (fn) | 内部把 `self.entries` 替换为 `self.history.entries()`；把 `self.truncate_entries_before(...)` 替换为 `self.history.truncate_before(...)` |
| `run_auto_compaction` (fn) | 把 `self.emit_event(...)` 替换为 `self.event_hub.emit(...)`；`self.entries.push(...)` → `self.history.append_compaction(...)`；`self.truncate_entries_before(...)` → `self.history.truncate_before(...)` |
| `run_with_messages` (fn) | 替换所有 self.entries / self.state / self.error_reason / self.event_tx / self.recovery / self.abort_token / self.last_saved_entry_count 字段访问为对应子系统调用（见 §6.2 简化示例） |
| `complete_with_deltas` (fn) | 把内部 event_sink 闭包改为使用 `self.event_hub.event_tx_clone()`；steer/follow_up 队列改为 `self.event_hub.steer_queue_clone()` / `.follow_up_queue_clone()` |

**注意**：删除内部不再使用的 imports（如 `std::sync::atomic::{AtomicU8, Ordering}`、`std::sync::Mutex`、`tokio_util::sync::CancellationToken`、`crate::harness::error_recovery::RecoveryStateMachine`、`tokio::task::JoinHandle` 等如果在删除字段后不再使用）。

- [ ] **Step 4: 编译验证**

Run:
```bash
cargo build -p agent-core 2>&1 | tail -20
```

Expected: `Finished`。如果有 borrow checker 冲突，按错误信息调整。

- [ ] **Step 5: 跑全部 SessionActor 测试**

Run:
```bash
cargo test -p agent-core --lib harness::session::tests 2>&1 | tail -10
```

Expected: 22 tests passed（old.rs 还在，所以测试代码无需改）。

- [ ] **Step 6: 跑下游 crate 编译**

```bash
cargo build -p tavern-comp 2>&1 | tail -10
```

Expected: `Finished`。

- [ ] **Step 7: Commit**

```bash
git add crates/agent-core/src/harness/session/old.rs
git commit -m "refactor(session): delegate SessionActor methods to subsystems (fields retained)"
```

---

### Task 6: 修改 2 个测试的私有字段访问 + 从 SessionActor 删除已迁移字段

**Files:**
- Modify: `crates/agent-core/src/harness/session/old.rs`（test 模块 + struct 字段）
- Modify: `crates/agent-core/src/harness/session/state.rs`（已实现）
- Modify: `crates/agent-core/src/harness/session/history.rs`（已实现）

- [ ] **Step 1: 修改 test_abort_session 的字段访问**

在 `old.rs` 的 test 模块中找到 `test_abort_session`（约 line 1938），把：

```rust
assert!(session.abort_token.is_cancelled());
```

改为：

```rust
assert!(session.state_machine.abort_token_ref().is_cancelled());
```

- [ ] **Step 2: 修改 test_entries_api_with_compaction 的字段访问**

在 `old.rs` 的 test 模块中找到 `test_entries_api_with_compaction`（约 line 2103），把：

```rust
session.entries.push(SessionEntry::Compaction { ... });
```

改为：

```rust
session.history.entries_mut().push(SessionEntry::Compaction { ... });
```

- [ ] **Step 3: 删除 16 个已迁移字段**

从 SessionActor struct 删除以下字段：

```rust
// ── 已迁移到 SessionHistory ──
entries: Vec<SessionEntry>,
store: Option<Arc<dyn SessionStore>>,
needs_restore: bool,
last_saved_entry_count: usize,
last_save: Option<tokio::task::JoinHandle<()>>,

// ── 已迁移到 SessionEventHub ──
event_listeners: Arc<Mutex<Vec<Arc<dyn AgentEventListener>>>>,
event_tx: Option<tokio::sync::mpsc::Sender<QueuedEvent>>,
event_processor_handle: Option<tokio::task::JoinHandle<()>>,
steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,

// ── 已迁移到 SessionStateMachine ──
state: AtomicU8,
error_reason: Mutex<Option<String>>,
recovery: RecoveryStateMachine,
abort_token: CancellationToken,

// ── 删除（dead code，无 reader）──
session_started_at: std::time::SystemTime,
```

**注意**：`QueuedEvent` 也已经在 event_hub.rs 定义（`pub(crate) struct QueuedEvent`）。old.rs 里如果还有 `struct QueuedEvent` 定义，要删除（Task 7 Step 1 会统一处理）。

- [ ] **Step 4: 修改 SessionActor 中剩余引用这些字段的方法**

按 Task 5 Step 3 表逐个修改，重点：

- `run_with_messages` / `complete_with_deltas` / `apply_context_strategy_before_run` / `run_auto_compaction` / `check_compaction` / `compact` / `prompt` / `prompt_with_content` / `continue_` 等所有引用 `self.entries`、`self.state`、`self.abort_token`、`self.error_reason`、`self.event_tx`、`self.recovery`、`self.last_saved_entry_count` 的方法。
- 用子系统方法替换：`self.state.enter_running()` / `self.state.enter_idle()` / `self.state.enter_error(reason)` / `self.state.clear_error()` / `self.state.child_token()` 等。

- [ ] **Step 5: 删除 unused imports**

Run:
```bash
cargo build -p agent-core 2>&1 | grep "warning.*unused" | head -20
```

逐个删除这些 imports。

- [ ] **Step 6: 编译验证**

Run:
```bash
cargo build -p agent-core 2>&1 | tail -20
```

Expected: `Finished`。

- [ ] **Step 7: 跑全部 SessionActor 测试**

Run:
```bash
cargo test -p agent-core --lib harness::session::tests 2>&1 | tail -15
```

Expected: 22 tests passed。如果失败，检查 Step 1/2 的测试修改是否正确。

- [ ] **Step 8: Commit**

```bash
git add crates/agent-core/src/harness/session/old.rs
git commit -m "refactor(session): remove 16 migrated fields from SessionActor + update 2 field-access tests"
```

---

### Task 7: 把 SessionActor 的代码从 old.rs 移到 mod.rs

**Files:**
- Modify: `crates/agent-core/src/harness/session/mod.rs`（接收 old.rs 内容）
- Delete: `crates/agent-core/src/harness/session/old.rs`

- [ ] **Step 1: 复制 old.rs 内容到 mod.rs 临时文件**

```bash
cp crates/agent-core/src/harness/session/old.rs crates/agent-core/src/harness/session/mod.rs
```

- [ ] **Step 2: 删除 mod.rs 中的 QueuedEvent struct 定义**

`mod.rs` 复制后包含 `struct QueuedEvent { event: AgentEvent }`（约 line 27）。这与 `event_hub.rs` 的 `pub(crate) struct QueuedEvent` 冲突。

删除 mod.rs 中的 `struct QueuedEvent { event: AgentEvent }` 块（约 3 行），保留 `mod event_hub; pub use event_hub::SessionEventHub;`（不重新导出 `QueuedEvent`，因为它是 `pub(crate)`）。

- [ ] **Step 3: 在 mod.rs 顶部加子模块声明**

在 mod.rs 顶部（imports 之前），确保有：

```rust
pub mod event_hub;
pub mod history;
pub mod state;

pub use event_hub::SessionEventHub;
pub use history::SessionHistory;
pub use state::SessionStateMachine;
```

如果 mod.rs 顶部有 `pub use old::*;`（从 Task 1 沿用），删除。

- [ ] **Step 4: 修复 mod.rs 中的内部路径**

mod.rs 现在直接拥有 SessionActor，可能有 `crate::harness::session::xxx` 路径引用。改为直接 `use xxx::xxx` 或 `crate::xxx`。

- [ ] **Step 5: 删除 old.rs**

```bash
git rm crates/agent-core/src/harness/session/old.rs
```

- [ ] **Step 6: 编译验证**

Run:
```bash
cargo build -p agent-core 2>&1 | tail -20
```

Expected: `Finished`。如果有 `QueuedEvent` 冲突或其他错误，按错误信息修复。

- [ ] **Step 7: 跑全部 SessionActor 测试**

Run:
```bash
cargo test -p agent-core --lib harness::session::tests 2>&1 | tail -10
```

Expected: 22 tests passed。

- [ ] **Step 8: Commit**

```bash
git add -A crates/agent-core/src/harness/session/
git commit -m "refactor(session): consolidate old.rs into mod.rs, remove old.rs"
```

---

## Phase 4：完整测试验证

### Task 8: 全 workspace 测试 + 下游 crate 验证

- [ ] **Step 1: 跑 agent-core 全部测试**

```bash
cargo test -p agent-core 2>&1 | tail -20
```

Expected: 22 + 5 + 6 + 4 = 37 tests passed（包含 subsystem tests）。

- [ ] **Step 2: 跑下游 crate 编译**

```bash
cargo build -p tavern-comp 2>&1 | tail -10
cargo build -p tenant 2>&1 | tail -10
cargo build -p api-gateway 2>&1 | tail -10
```

Expected: 全部 `Finished`。

- [ ] **Step 3: 跑下游 crate 测试**

```bash
cargo test -p tavern-comp --lib 2>&1 | tail -10
cargo test -p tenant --lib 2>&1 | tail -10
```

Expected: 全部通过。

- [ ] **Step 4: 如有失败，定位修复**

如失败，对照 §6.3 委托映射表确认 API 兼容性。tenant crate 用 `actor.abort_token()`、`actor.state()`、`actor.error_reason()` 公开方法，不应受影响。

- [ ] **Step 5: Commit（如有修复）**

```bash
git add crates/
git commit -m "fix: downstream crate compat after SessionActor refactor"
```

---

## Phase 5：清理与验证

### Task 9: 行数 + clippy + doc 验证

- [ ] **Step 1: 检查行数**

```bash
wc -l crates/agent-core/src/harness/session/*.rs
```

Expected:
- `mod.rs` ≤ 800
- `history.rs` ≤ 300
- `event_hub.rs` ≤ 250
- `state.rs` ≤ 200

- [ ] **Step 2: clippy 检查**

```bash
cargo clippy -p agent-core -- -D warnings 2>&1 | tail -20
```

Expected: 无 warning。

- [ ] **Step 3: doc 检查**

```bash
cargo doc -p agent-core --no-deps 2>&1 | tail -10
```

Expected: 无 broken link。

- [ ] **Step 4: 跑全部 workspace 测试**

```bash
cargo test --workspace 2>&1 | tail -20
```

Expected: 全部通过。

- [ ] **Step 5: 如有警告，修复并 commit**

```bash
git add crates/
git commit -m "chore: clippy + doc cleanup after SessionActor refactor"
```

---

### Task 10: 更新 AGENTS.md

**Files:**
- Modify: `AGENTS.md`

- [ ] **Step 1: 更新当前状态表的"代码质量"行**

找到"代码质量"行，更新为：

```markdown
| 代码质量 | ✅ 生产代码零 unwrap；SessionActor 已拆分（history.rs / event_hub.rs / state.rs / mod.rs），22 个原有测试保持不变通过 + 15 个新子系统测试 |
```

（15 = 5 SessionStateMachine + 6 SessionHistory + 4 SessionEventHub）

- [ ] **Step 2: Commit**

```bash
git add AGENTS.md
git commit -m "docs(agents): update status table for SessionActor refactor"
```

---

## 验收清单

- [ ] Task 1-10 全部完成
- [ ] `wc -l crates/agent-core/src/harness/session/*.rs` 满足行数目标
- [ ] `cargo test -p agent-core` 全过（22 + 15 = 37 个测试）
- [ ] `cargo build -p tavern-comp -p tenant -p api-gateway` 全过
- [ ] `cargo clippy -p agent-core -- -D warnings` 无 warning
- [ ] `cargo doc -p agent-core --no-deps` 无 broken link
- [ ] AGENTS.md 已更新

## 风险与回滚

| 风险 | 回滚 |
|---|---|
| 委托化阶段 borrow checker 冲突 | 保留原字段（Task 5），仅改方法体 |
| 下游 crate 测试失败 | 公共 API 100% 兼容；私有字段访问仅限 agent-core 内部测试 |
| Task 7 QueuedEvent 冲突 | Step 2 明确删除 mod.rs 中的 `struct QueuedEvent` 定义 |
| `persist_incremental` 改为 async 后 caller 忘了 `.await` | Task 6 Step 4 列出所有 caller，确保都加 `.await` |
| clippy 警告（dead_code、must_use） | 按需 `#[allow(...)]` 标注 |
| Phase 3 删除字段后编译错误 | `git revert` 到上一 commit，定位错误后重新 apply |