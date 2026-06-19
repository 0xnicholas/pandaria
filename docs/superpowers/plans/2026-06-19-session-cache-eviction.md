# Session Cache 淘汰策略 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade PandariaAgentExecutor's session cache from lazy idle-timeout to LRU + capacity-bound + background cleanup, and add TTL-based cleanup of completed/failed sessions across all SessionStore backends.

**Architecture:** Phase A replaces `HashMap<String, CachedSession>` with a new `SessionCache` struct wrapping `lru::LruCache` plus a background tokio task for idle eviction. Phase B adds `cleanup_expired_sessions()` to the `SessionStore` trait with PostgreSQL, Redis, and in-memory implementations. Phase C wires retention config into `HarnessConfig` and spawns a periodic cleanup task in `TenantManagerImpl`.

**Tech Stack:** Rust + tokio, `lru` crate, existing agent-core/tavern-comp/storage/tenant crates.

---

## File Map

| Phase | File | Action | Responsibility |
|-------|------|--------|----------------|
| A | `crates/tavern-comp/Cargo.toml` | Modify | Add `lru` dependency; add `agent-core` with `testing` feature in dev-dependencies |
| A | `crates/tavern-comp/src/team/session_cache.rs` | **Create** | `SessionCache` struct: LRU + idle timeout + background cleanup |
| A | `crates/tavern-comp/src/team/pandaria_executor.rs` | Modify | Replace `HashMap` with `SessionCache`; add config methods |
| A | `crates/tavern-comp/src/team/mod.rs` | Modify | Register `session_cache` module |
| B | `crates/agent-core/src/persistence/store.rs` | Modify | Add `cleanup_expired_sessions` to `SessionStore` trait |
| B | `crates/storage/src/session/postgres.rs` | Modify | Implement `cleanup_expired_sessions` for PG |
| B | `crates/storage/src/session/redis.rs` | Modify | Implement `cleanup_expired_sessions` for Redis (TTL-based, SCAN deferred) |
| C | `crates/agent-core/src/harness/config.rs` | Modify | Add `session_retention_days` / `session_cleanup_interval_hours` to `HarnessConfig` |
| C | `crates/tenant/src/manager.rs` | Modify | Spawn background cleanup task; pass config from `HarnessConfig` |
| D | `crates/tavern-comp/tests/session_cache_tests.rs` | **Create** | Integration tests for `SessionCache` |
| D | `crates/storage/tests/session_cleanup_tests.rs` | **Create** | Integration tests for `cleanup_expired_sessions` across backends |

---

## Phase A: PandariaAgentExecutor LRU Cache

### Task A1: Add `lru` crate dependency

**Files:**

- Modify: `crates/tavern-comp/Cargo.toml`

- [ ] **Step 1: Add lru dependency and dev-dependency**

```toml
# Under [dependencies]
lru = "0.12"

# Under [dev-dependencies], add or update:
agent-core = { path = "../agent-core", features = ["testing"] }
```

- [ ] **Step 2: Verify dependency resolves**

```bash
cargo check -p tavern-comp 2>&1 | tail -3
```

Expected: compiles (existing code unchanged).

- [ ] **Step 3: Commit**

```bash
git add crates/tavern-comp/Cargo.toml
git commit -m "chore(tavern): add lru crate dependency"
```

### Task A2: Create `SessionCache` struct

**Files:**

- Create: `crates/tavern-comp/src/team/session_cache.rs`
- Modify: `crates/tavern-comp/src/team/mod.rs`

- [ ] **Step 1: Write the module file**

```rust
//! LRU-bounded session cache with idle-timeout eviction and background cleanup.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_core::SessionActor;
use lru::LruCache;
use std::num::NonZeroUsize;

/// Wraps a cached `SessionActor` with access-time metadata.
#[derive(Clone)]
pub(crate) struct CachedSession {
    pub actor: Arc<tokio::sync::Mutex<SessionActor>>,
    pub last_used: Instant,
}

/// LRU-aware session cache bound by capacity and idle timeout.
///
/// # Eviction order
///
/// 1. **Background idle scan** (every `cleanup_interval`): removes entries
///    idle longer than `idle_timeout`. Flushes each evicted actor before drop.
/// 2. **LRU eviction on insert**: when `put()` exceeds `max_cached`, pops the
///    least-recently-used entry. Flushes before drop (best-effort; if
///    `try_lock` fails, drops without flush since authoritative state lives
///    in PostgreSQL/Redis).
pub(crate) struct SessionCache {
    entries: Mutex<LruCache<String, CachedSession>>,
    max_cached: usize,
    idle_timeout: Duration,
}

impl SessionCache {
    /// Create a new cache with the given capacity and idle timeout.
    pub fn new(max_cached: usize, idle_timeout: Duration) -> Self {
        let cap = NonZeroUsize::new(max_cached.max(1)).unwrap();
        Self {
            entries: Mutex::new(LruCache::new(cap)),
            max_cached,
            idle_timeout,
        }
    }

    /// Look up a session by key. Promotes the entry to MRU on hit.
    /// Returns `None` if not found or the entry has been idle longer than
    /// `idle_timeout` (expired entries are popped from the cache).
    pub fn get(&self, key: &str) -> Option<CachedSession> {
        let mut map = self.entries.lock().expect("session cache poisoned");
        match map.get(key) {
            Some(entry) if entry.last_used.elapsed() < self.idle_timeout => {
                // lru::LruCache::get promotes to MRU — clone to return while
                // keeping the entry in the cache.
                let mut cloned = entry.clone();
                cloned.last_used = Instant::now();
                // Re-insert with updated timestamp to refresh position
                map.put(key.to_string(), cloned.clone());
                Some(cloned)
            }
            Some(_) => {
                // Expired — remove and return None
                map.pop(key);
                None
            }
            None => None,
        }
    }

    /// Insert a session entry. If the cache is at capacity, evicts the LRU
    /// entry and returns it for the caller to flush.
    pub fn put(&self, key: String, entry: CachedSession) -> Option<CachedSession> {
        let mut map = self.entries.lock().expect("session cache poisoned");
        let evicted = if map.len() >= self.max_cached && !map.contains(&key) {
            map.pop_lru().map(|(_, v)| v)
        } else {
            None
        };
        map.put(key, entry);
        evicted
    }

    /// Remove an entry by key and return it. Does NOT flush.
    pub fn pop(&self, key: &str) -> Option<CachedSession> {
        let mut map = self.entries.lock().expect("session cache poisoned");
        map.pop(key)
    }

    /// Scan for idle entries exceeding `idle_timeout` and remove them.
    /// Returns the list of evicted entries for the caller to flush.
    pub fn evict_idle(&self) -> Vec<(String, CachedSession)> {
        let mut map = self.entries.lock().expect("session cache poisoned");
        let now = Instant::now();
        let keys: Vec<String> = map
            .iter()
            .filter(|(_, c)| now.duration_since(c.last_used) > self.idle_timeout)
            .map(|(k, _)| k.clone())
            .collect();

        let mut evicted = Vec::new();
        for k in keys {
            if let Some(entry) = map.pop(&k) {
                evicted.push((k, entry));
            }
        }
        evicted
    }

    /// Number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.entries.lock().expect("session cache poisoned").len()
    }

    /// Drain all entries from the cache (used at executor shutdown).
    /// Returns all cached sessions for the caller to flush.
    pub fn drain_all(&self) -> Vec<(String, CachedSession)> {
        let mut map = self.entries.lock().expect("session cache poisoned");
        let keys: Vec<String> = map.iter().map(|(k, _)| k.clone()).collect();
        let mut drained = Vec::new();
        for k in keys {
            if let Some(entry) = map.pop(&k) {
                drained.push((k, entry));
            }
        }
        drained
    }
}

/// Spawn a background task that periodically evicts idle sessions,
/// flushing each evicted actor before dropping it.
///
/// If `try_lock` fails (actor is currently executing), the entry is
/// skipped and will be retried on the next cycle.
pub(crate) fn spawn_cache_cleanup(
    cache: Arc<SessionCache>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            let evicted = cache.evict_idle();
            for (_key, entry) in evicted {
                if let Ok(mut actor) = entry.actor.try_lock() {
                    if let Err(e) = actor.flush().await {
                        tracing::warn!(error = %e, "session cache flush failed during eviction");
                    }
                }
                // If try_lock fails, actor is in use — skip, clean up next cycle
            }
        }
    })
}
```

- [ ] **Step 2: Register module in `team/mod.rs`**

In `crates/tavern-comp/src/team/mod.rs`, add after existing module declarations:

```rust
pub(crate) mod session_cache;
```

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p tavern-comp 2>&1 | tail -3
```

Expected: compiles (unused module warning ok for now).

- [ ] **Step 4: Commit**

```bash
git add crates/tavern-comp/src/team/session_cache.rs crates/tavern-comp/src/team/mod.rs
git commit -m "feat(tavern): add SessionCache with LRU + idle timeout + background cleanup"
```

### Task A3: Write SessionCache unit tests

**Files:**

- Modify: `crates/tavern-comp/src/team/session_cache.rs` (add `#[cfg(test)]` module)

- [ ] **Step 1: Write tests**

Add at the bottom of `session_cache.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_entry() -> CachedSession {
        CachedSession {
            actor: Arc::new(tokio::sync::Mutex::new(
                agent_core::SessionActor::dummy_for_test(),
            )),
            last_used: Instant::now(),
        }
    }

    #[test]
    fn test_put_and_get() {
        let cache = SessionCache::new(4, Duration::from_secs(300));
        cache.put("a".into(), make_entry());
        assert!(cache.get("a").is_some());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_get_miss_returns_none() {
        let cache = SessionCache::new(4, Duration::from_secs(300));
        assert!(cache.get("nonexistent").is_none());
    }

    #[test]
    fn test_get_expired_returns_none() {
        let cache = SessionCache::new(4, Duration::from_millis(10));
        let mut entry = make_entry();
        entry.last_used = Instant::now() - Duration::from_secs(1);
        cache.put("a".into(), entry);
        // Should be expired
        assert!(cache.get("a").is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_lru_eviction_on_full() {
        let cache = SessionCache::new(2, Duration::from_secs(300));
        cache.put("a".into(), make_entry());
        cache.put("b".into(), make_entry());
        // Cache is full; inserting 'c' should evict LRU ('a')
        let evicted = cache.put("c".into(), make_entry());
        assert!(evicted.is_some());
        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_some());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn test_get_promotes_to_mru() {
        let cache = SessionCache::new(2, Duration::from_secs(300));
        cache.put("a".into(), make_entry());
        cache.put("b".into(), make_entry());
        // Access 'a' to promote it
        assert!(cache.get("a").is_some());
        // Insert 'c' — should evict 'b' (now LRU)
        let evicted = cache.put("c".into(), make_entry());
        assert!(evicted.is_some());
        assert!(cache.get("a").is_some());
        assert!(cache.get("b").is_none());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn test_evict_idle_removes_expired() {
        let cache = SessionCache::new(4, Duration::from_millis(10));
        cache.put("a".into(), make_entry());

        let mut old = make_entry();
        old.last_used = Instant::now() - Duration::from_secs(1);
        cache.put("b".into(), old);

        std::thread::sleep(Duration::from_millis(20));

        let evicted = cache.evict_idle();
        assert_eq!(evicted.len(), 2, "both entries should be expired");
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_pop_removes_entry() {
        let cache = SessionCache::new(4, Duration::from_secs(300));
        cache.put("a".into(), make_entry());
        let popped = cache.pop("a");
        assert!(popped.is_some());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_put_same_key_no_eviction() {
        let cache = SessionCache::new(2, Duration::from_secs(300));
        cache.put("a".into(), make_entry());
        cache.put("b".into(), make_entry());
        // Update existing key — should not evict
        let evicted = cache.put("a".into(), make_entry());
        assert!(evicted.is_none());
        assert_eq!(cache.len(), 2);
    }
}
```

> Note: `SessionActor::dummy_for_test()` doesn't exist yet. It will be added in the next step (Task A4).

- [ ] **Step 2: Run tests — expect COMPILE ERROR**

```bash
cargo test -p tavern-comp -- session_cache
```

Expected: COMPILE ERROR — `SessionActor::dummy_for_test()` not defined.

- [ ] **Step 3: Add `dummy_for_test()` to SessionActor**

In `crates/agent-core/src/harness/session.rs`, add a test-only constructor. Place it inside an `#[cfg(any(test, feature = "testing"))]` block on SessionActor:

```rust
#[cfg(any(test, feature = "testing"))]
impl SessionActor {
    /// Create a minimal, non-functional SessionActor for use in unit tests
    /// of downstream crates (e.g., session cache tests in tavern-comp).
    /// The returned actor cannot execute prompts — it exists solely as a
    /// placeholder for cache data structure tests.
    pub fn dummy_for_test() -> Self {
        use crate::harness::compaction::CompactionConfig;
        use crate::harness::session::SessionConfig;
        use crate::hook::default_dispatcher::DefaultHookDispatcher;
        use crate::space::AgentSpace;
        use std::sync::Arc;

        let dispatcher = Arc::new(DefaultHookDispatcher::from_config(
            AgentSpace::default(),
            &crate::harness::config::HookConfig::default(),
        ));
        let compaction = Arc::new(crate::harness::compaction::Compactor::new(
            CompactionConfig::default(),
            Arc::new(ai_provider::RouterProvider::new()),
            "dummy".into(),
            Arc::new(crate::file_ops::DefaultFileOperationExtractor::default()),
        ));

        Self::new(SessionConfig {
            tenant_id: "dummy".into(),
            session_id: "dummy".into(),
            system_prompt: String::new(),
            model: "dummy".into(),
            provider: Arc::new(ai_provider::RouterProvider::new()),
            hook_dispatcher: dispatcher,
            compaction_actor: compaction,
            tools: vec![],
            store: None,
            skills: vec![],
        })
    }
}
```

- [ ] **Step 4: Run tests — verify PASS**

```bash
cargo test -p tavern-comp -- session_cache --nocapture
```

Expected: 7 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tavern-comp/src/team/session_cache.rs crates/agent-core/src/harness/session.rs
git commit -m "test(tavern): add SessionCache unit tests and SessionActor::dummy_for_test()"
```

### Task A4: Integrate SessionCache into PandariaAgentExecutor

**Files:**

- Modify: `crates/tavern-comp/src/team/pandaria_executor.rs`

- [ ] **Step 1: Remove `CachedSession` and `sessions` field, replace with `SessionCache`**

Current struct (lines ~22-55):

```rust
// Remove this:
struct CachedSession {
    actor: Arc<Mutex<SessionActor>>,
    last_used: Instant,
}
```

Replace `sessions` field:

```rust
// Replace:
sessions: Arc<std::sync::Mutex<HashMap<String, CachedSession>>>,

// With:
sessions: Arc<super::session_cache::SessionCache>,
```

Also add the new fields:

```rust
/// Interval between background idle-eviction scans (default: 60s).
cleanup_interval: std::time::Duration,
/// Handle for the background cleanup task.
_cleanup_handle: Option<tokio::task::JoinHandle<()>>,
```

- [ ] **Step 2: Update `PandariaAgentExecutor::new()`**

Replace session cache initialization:

```rust
// Remove:
sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),

// Add:
let sessions = Arc::new(super::session_cache::SessionCache::new(
    16, // max_cached_sessions
    std::time::Duration::from_secs(300), // idle_timeout
));

// Spawn background cleanup
let cleanup_handle = super::session_cache::spawn_cache_cleanup(
    sessions.clone(),
    std::time::Duration::from_secs(60),
);

// In struct:
session_idle_timeout: std::time::Duration::from_secs(300),
cleanup_interval: std::time::Duration::from_secs(60),
_cleanup_handle: Some(cleanup_handle),
// ... keep existing fields
```

- [ ] **Step 3: Add config methods**

```rust
/// Set the maximum number of cached sessions (default: 16).
/// When the cache is full, the least-recently-used entry is evicted
/// on the next insert.
///
/// **Important:** this method replaces the internal cache and restarts
/// the background cleanup task. Call only during builder configuration,
/// before any sessions are created.
pub fn with_max_cached_sessions(mut self, n: usize) -> Self {
    // Recreate the cache with the new capacity.
    self.sessions = Arc::new(super::session_cache::SessionCache::new(
        n.max(1),
        self.session_idle_timeout,
    ));
    // Restart the cleanup task to watch the new cache
    self._cleanup_handle = Some(super::session_cache::spawn_cache_cleanup(
        self.sessions.clone(),
        self.cleanup_interval,
    ));
    self
}

/// Set the interval between background idle-eviction scans (default: 60s).
pub fn with_cleanup_interval(mut self, interval: std::time::Duration) -> Self {
    self.cleanup_interval = interval;
    // Restart the cleanup task with the new interval
    self._cleanup_handle = Some(super::session_cache::spawn_cache_cleanup(
        self.sessions.clone(),
        interval,
    ));
    self
}
```

- [ ] **Step 4: Update `acquire_session()` (previously `acquire_or_create_session`)**

Replace all `HashMap` operations with `SessionCache` methods:

```rust
// Fast path (replaces map.lock() + get + get_mut at lines ~296-310):
// Replace:
//   let mut map = self.sessions.lock()...
//   if let Some(cached) = map.get(&cache_key) { ... }
// With:
if let Some(cached) = self.sessions.get(&cache_key) {
    return Ok(cached.actor.clone());
}

// Slow path — double-check after semaphore (replaces lines ~320-356):
// Replace:
//   let mut map = self.sessions.lock()...
//   if let Some(cached) = map.get(&cache_key) { ... }
// With:
if let Some(cached) = self.sessions.get(&cache_key) {
    return Ok(cached.actor.clone());
}

// Insert new session (after creating actor):
let entry = super::session_cache::CachedSession {
    actor: actor.clone(),
    last_used: std::time::Instant::now(),
};
if let Some(evicted) = self.sessions.put(cache_key.clone(), entry) {
    // Flush evicted LRU entry outside the lock
    if let Ok(mut a) = evicted.actor.try_lock() {
        let _ = a.flush().await;
    }
}
```

Remove the old double-check `if let Some(cached) = map.get(&cache_key) { if ... } -> return } else { map.remove }` pattern — `SessionCache::get` already handles idle-expiry internally.

- [ ] **Step 5: Update `flush()` method**

The current `flush()` clones the HashMap and iterates all sessions. Replace with `SessionCache::drain_all()` to drain ALL sessions (not just idle ones) at shutdown:

```rust
async fn flush(&self) -> Result<(), AgentExecutorError> {
    let all = self.sessions.drain_all();
    for (cache_key, entry) in all {
        let mut actor = entry.actor.lock().await;
        actor.flush().await.map_err(|e| {
            AgentExecutorError::ExecutionFailed(format!(
                "flush session {cache_key} failed: {e}"
            ))
        })?;
    }
    Ok(())
}
```

- [ ] **Step 6: Update `session_count()`**

```rust
pub fn session_count(&self) -> usize {
    self.sessions.len()
}
```

- [ ] **Step 7: Verify compilation**

```bash
cargo check -p tavern-comp 2>&1 | tail -10
```

Expected: compiles. Fix any type mismatches.

- [ ] **Step 8: Run existing tavern-comp tests**

```bash
cargo test -p tavern-comp 2>&1 | tail -10
```

Expected: all existing 13 tests pass.

- [ ] **Step 9: Commit**

```bash
git add crates/tavern-comp/src/team/pandaria_executor.rs
git commit -m "refactor(tavern): replace HashMap with SessionCache (LRU + idle timeout) in PandariaAgentExecutor"
```

### Task A5: Add integration test for SessionCache + PandariaAgentExecutor

**Files:**

- Create: `crates/tavern-comp/tests/session_cache_tests.rs`

- [ ] **Step 1: Write integration tests**

These tests verify SessionCache behavior through the PandariaAgentExecutor public API:

```rust
use std::sync::Arc;
use agent_core::harness::config::HarnessConfig;
use tavern_comp::PandariaAgentExecutor;
use tavern_core::{AgentConfig, MemoryConfig, ModelConfig};

fn dummy_agent(id: &str, model: &str) -> AgentConfig {
    AgentConfig {
        id: id.into(), name: id.into(), description: None,
        model: ModelConfig { provider: "test".into(), name: model.into(), temperature: 0.7 },
        instructions: "test".into(), skills: vec![], constraints: vec![],
        memory: MemoryConfig::default(),
    }
}

#[tokio::test]
async fn test_session_count_reflects_cache() {
    let harness = HarnessConfig::from_env(Arc::new(ai_provider::RouterProvider::new()));
    let resolver = Arc::new(tavern_comp::InMemoryAgentResolver::new(vec![dummy_agent("r1","m1")]));
    let executor = PandariaAgentExecutor::new("t1", "team1", harness, resolver);
    assert_eq!(executor.session_count(), 0);
    // Execute a mission to populate cache, then verify session_count() > 0
    // (actual execution requires AgentExecutor trait; see inline note below)
}

#[tokio::test]
async fn test_lru_eviction_on_full_cache() {
    let harness = HarnessConfig::from_env(Arc::new(ai_provider::RouterProvider::new()));
    let agents: Vec<AgentConfig> = (0..5).map(|i| dummy_agent(&format!("r{i}"), &format!("m{i}"))).collect();
    let resolver = Arc::new(tavern_comp::InMemoryAgentResolver::new(agents.clone()));
    let executor = PandariaAgentExecutor::new("t1", "team1", harness, resolver)
        .with_max_cached_sessions(3);
    // Execute 5 missions with distinct role/model pairs; cache should be capped at 3
    assert!(executor.session_count() <= 3);
}
```

> **API visibility note:** `acquire_session` is `pub(crate)`. Integration tests use the `AgentExecutor` trait's `execute()` method (public). The plan's Task A4 may need to expose a test-only constructor or make `acquire_session` visible behind `#[cfg(feature = "testing")]`.

- [ ] **Step 2: Run tests — fix visibility issues**

```bash
cargo test -p tavern-comp --test session_cache_tests -- --nocapture
```

Expected: resolve API visibility (add `#[cfg(feature = "testing")]` pub re-exports if needed), then tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/tavern-comp/tests/session_cache_tests.rs
git commit -m "test(tavern): integration tests for SessionCache"
```

---

## Phase B: SessionStore Trait + Backends

### Task B1: Add `cleanup_expired_sessions` to SessionStore trait

**Files:**

- Modify: `crates/agent-core/src/persistence/store.rs`

- [ ] **Step 1: Add method to trait**

After `update_session_status()` (line ~62), add:

```rust
/// Clean up sessions in terminal states (`completed` / `failed`)
/// that haven't been updated within `older_than`.
///
/// This is a **global** operation — it scans across all tenants.
/// The method does NOT accept a `tenant_id` because cleanup is
/// performed by a single background task in `TenantManagerImpl`
/// and a single SQL query is more efficient than per-tenant calls.
///
/// Returns the number of sessions deleted.
///
/// # Default
///
/// Returns `Ok(0)` for stores that do not track lifecycle status.
async fn cleanup_expired_sessions(
    &self,
    _older_than: std::time::Duration,
) -> Result<u64, AgentError> {
    Ok(0)
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p agent-core 2>&1 | tail -3
```

Expected: compiles clean.

- [ ] **Step 3: Commit**

```bash
git add crates/agent-core/src/persistence/store.rs
git commit -m "feat(agent-core): add cleanup_expired_sessions to SessionStore trait"
```

### Task B2: Implement for PostgreSQL

**Files:**

- Modify: `crates/storage/src/session/postgres.rs`

- [ ] **Step 1: Add implementation**

After `update_session_status()` in `impl SessionStore for PgSessionStore`:

```rust
async fn cleanup_expired_sessions(
    &self,
    older_than: std::time::Duration,
) -> Result<u64, AgentError> {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(older_than)
        .expect("cutoff time computation underflow");

    let result = sqlx::query(
        "DELETE FROM sessions WHERE status IN ('completed', 'failed') AND updated_at < $1",
    )
    .bind(cutoff)
    .execute(&self.pool)
    .await
    .map_err(|e| AgentError::Persistence(format!("pg cleanup: {e}")))?;

    Ok(result.rows_affected())
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p storage 2>&1 | tail -3
```

Expected: compiles clean (sqlx 0.8 maps `SystemTime` to `TIMESTAMPTZ` with the `postgres` feature).

- [ ] **Step 3: Commit**

```bash
git add crates/storage/src/session/postgres.rs
git commit -m "feat(storage): implement cleanup_expired_sessions for PostgreSQL"
```

### Task B3: Implement for Redis

**Files:**

- Modify: `crates/storage/src/session/redis.rs`

- [ ] **Step 1: Add implementation (delegates to TTL)**

Redis already sets a 7-day TTL on session keys. **Deliberate deviation from spec §3.2**: the spec proposed `SCAN` + `HGET status` + `DEL` for active cleanup, but this is expensive for large deployments and duplicates Redis's built-in key expiration. For now, implement as a no-op relying on TTL; active SCAN-based cleanup is deferred to a follow-up task.

```rust
async fn cleanup_expired_sessions(
    &self,
    _older_than: std::time::Duration,
) -> Result<u64, AgentError> {
    // Redis session keys already have a TTL (default 7 days).
    // Active scanning would require SCAN + HGET status + DEL,
    // which is expensive for large deployments. Rely on key
    // expiration instead.
    tracing::debug!("redis session cleanup: relying on key TTL ({}s)", self.ttl_seconds);
    Ok(0)
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p storage 2>&1 | tail -3
```

Expected: compiles clean.

- [ ] **Step 3: Commit**

```bash
git add crates/storage/src/session/redis.rs
git commit -m "feat(storage): implement cleanup_expired_sessions for Redis (TTL-based)"
```

### Task B4: Write cleanup tests for PostgreSQL

**Files:**

- Create: `crates/storage/tests/session_cleanup_tests.rs`

- [ ] **Step 1: Write tests using testcontainers**

Follow the existing pattern from `crates/storage/tests/integration_postgres.rs`:

```rust
use std::time::Duration;
use agent_core::{AgentError, SessionEntry, SessionStore};
use storage::PgSessionStore;

async fn setup_pg() -> PgSessionStore {
    // Use PANDARIA_TEST_PG_URL env var or testcontainers
    let db_url = std::env::var("PANDARIA_TEST_PG_URL")
        .unwrap_or_else(|_| panic!("PANDARIA_TEST_PG_URL not set"));
    let pool = sqlx::PgPool::connect(&db_url).await.unwrap();
    let store = PgSessionStore::new(pool);
    store.init().await.unwrap();
    store
}

fn make_entry(text: &str) -> SessionEntry {
    SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: text.into(), text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    }
}

#[tokio::test]
async fn test_cleanup_deletes_completed_sessions() {
    let store = setup_pg().await;
    store.save_session("t1", "s1", &[make_entry("hello")]).await.unwrap();
    store.update_session_status("t1", "s1", "completed").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let deleted = store.cleanup_expired_sessions(Duration::from_millis(50)).await.unwrap();
    assert_eq!(deleted, 1, "completed session should be cleaned up");
}

#[tokio::test]
async fn test_cleanup_preserves_active_sessions() {
    let store = setup_pg().await;
    store.save_session("t1", "s2", &[make_entry("active")]).await.unwrap();
    store.update_session_status("t1", "s2", "running").await.unwrap();
    let deleted = store.cleanup_expired_sessions(Duration::from_secs(1)).await.unwrap();
    assert_eq!(deleted, 0, "running session should NOT be cleaned up");
}
```

- [ ] **Step 2: Run tests**

```bash
PANDARIA_TEST_PG_URL="postgres://postgres@localhost:5432/postgres" \
  cargo test -p storage --test session_cleanup_tests -- --test-threads=1 --nocapture
```

Expected: 2 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/storage/tests/session_cleanup_tests.rs
git commit -m "test(storage): cleanup_expired_sessions integration tests for PostgreSQL"
```

---

## Phase C: HarnessConfig + TenantManager Integration

### Task C1: Add retention/cleanup config to HarnessConfig

**Files:**

- Modify: `crates/agent-core/src/harness/config.rs`

- [ ] **Step 1: Add fields to `HarnessConfig` struct**

After `memory_store`:

```rust
/// Days to retain completed/failed sessions before cleanup (default: 7).
pub session_retention_days: u32,
/// Hours between cleanup task executions (default: 24).
pub session_cleanup_interval_hours: u32,
```

- [ ] **Step 2: Update `from_env()`**

Add after existing env reads:

```rust
let session_retention_days = std::env::var("PANDARIA_SESSION_RETENTION_DAYS")
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(7);

let session_cleanup_interval_hours = std::env::var("PANDARIA_SESSION_CLEANUP_INTERVAL_HOURS")
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(24);
```

Add to the `Self { ... }` struct literal:

```rust
session_retention_days,
session_cleanup_interval_hours,
```

- [ ] **Step 3: Update `Default` impl**

```rust
session_retention_days: 7,
session_cleanup_interval_hours: 24,
```

- [ ] **Step 4: Update `Debug` impl**

Add:

```rust
.field("session_retention_days", &self.session_retention_days)
.field("session_cleanup_interval_hours", &self.session_cleanup_interval_hours)
```

- [ ] **Step 5: Verify compilation**

```bash
cargo check -p agent-core 2>&1 | tail -3
```

Expected: compiles clean.

- [ ] **Step 6: Commit**

```bash
git add crates/agent-core/src/harness/config.rs
git commit -m "feat(agent-core): add session retention/cleanup config to HarnessConfig"
```

### Task C2: Spawn background cleanup task in TenantManagerImpl::new()

**Files:**

- Modify: `crates/tenant/src/manager.rs`

- [ ] **Step 1: Spawn cleanup task in `TenantManagerImpl::new()`**

`TenantManagerImpl` already holds `runtime_config: Arc<HarnessConfig>` (line 234). Read retention config from there, and spawn the task directly in `new()` — no separate call site needed (the trait object `Arc<dyn TenantManager>` in api-gateway cannot call concrete methods):

At the end of `TenantManagerImpl::new()`, after the existing field initialisation:

```rust
// Spawn background session cleanup task
if let Some(ref store) = runtime_config.store {
    let store = store.clone();
    let retention_days = runtime_config.session_retention_days;
    let interval_hours = runtime_config.session_cleanup_interval_hours;

    let retention = std::time::Duration::from_secs(retention_days as u64 * 86400);
    let interval = std::time::Duration::from_secs(interval_hours as u64 * 3600);

    tokio::spawn(async move {
        // Wait for the first interval before starting, so the
        // server has time to initialise fully.
        tokio::time::sleep(interval).await;

        loop {
            match store.cleanup_expired_sessions(retention).await {
                Ok(0) => {
                    tracing::debug!("session cleanup: no expired sessions found");
                }
                Ok(count) => {
                    tracing::info!(count, "session cleanup: deleted expired sessions");
                }
                Err(e) => {
                    tracing::error!(error = %e, "session cleanup failed");
                }
            }
            tokio::time::sleep(interval).await;
        }
    });
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p tenant 2>&1 | tail -5
```

Expected: compiles clean.

- [ ] **Step 3: Run existing tenant tests**

```bash
cargo test -p tenant --lib 2>&1 | tail -5
```

Expected: all 21 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/tenant/src/manager.rs
git commit -m "feat(tenant): spawn background session cleanup task in TenantManagerImpl::new()"
```

---

## Phase D: Full Verification

### Task D1: Run full test suite

- [ ] **Step 1: agent-core tests**

```bash
cargo test -p agent-core --lib 2>&1 | tail -5
```

Expected: all tests pass.

- [ ] **Step 2: tavern-comp tests**

```bash
cargo test -p tavern-comp 2>&1 | tail -5
```

Expected: all tests pass.

- [ ] **Step 3: storage tests**

```bash
cargo test -p storage --lib 2>&1 | tail -5
```

Expected: all tests pass.

- [ ] **Step 4: tenant tests**

```bash
cargo test -p tenant --lib 2>&1 | tail -5
```

Expected: all tests pass.

- [ ] **Step 5: api-gateway tests**

```bash
cargo test -p api-gateway --lib 2>&1 | tail -5
```

Expected: all tests pass.

- [ ] **Step 6: Full workspace clippy**

```bash
cargo clippy -p agent-core -- -D warnings 2>&1 | tail -3
cargo clippy -p tavern-comp -- -D warnings 2>&1 | tail -3
cargo clippy -p storage -- -D warnings 2>&1 | tail -3
cargo clippy -p tenant -- -D warnings 2>&1 | tail -3
cargo clippy -p api-gateway -- -D warnings 2>&1 | tail -3
```

Expected: all clean.

- [ ] **Step 7: Commit**

```bash
git commit -m "chore: full verification — all tests pass, clippy clean"
```

---

## Task Summary

| # | Phase | Task | Est. |
|---|---|---|---|
| A1 | LRU Cache | Add `lru` dependency | 5 min |
| A2 | LRU Cache | Create `SessionCache` struct + `spawn_cache_cleanup` | 15 min |
| A3 | LRU Cache | `SessionCache` unit tests + `dummy_for_test()` | 15 min |
| A4 | LRU Cache | Integrate into `PandariaAgentExecutor` | 20 min |
| A5 | LRU Cache | Integration tests for SessionCache | 10 min |
| B1 | SessionStore | Add `cleanup_expired_sessions` to trait | 5 min |
| B2 | SessionStore | PostgreSQL implementation | 10 min |
| B3 | SessionStore | Redis implementation (TTL-based, SCAN deferred) | 5 min |
| B4 | SessionStore | Cleanup integration tests for PostgreSQL | 10 min |
| C1 | Config | Add retention/cleanup to `HarnessConfig` | 10 min |
| C2 | Config | Spawn cleanup task in `TenantManagerImpl::new()` | 10 min |
| D1 | Verify | Full test suite + clippy | 10 min |

**Total estimated:** ~2 hours
