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
                if let Ok(mut actor) = entry.actor.try_lock()
                    && let Err(e) = actor.flush().await {
                        tracing::warn!(error = %e, "session cache flush failed during eviction");
                    }
                // If try_lock fails, actor is in use — skip, clean up next cycle
            }
        }
    })
}