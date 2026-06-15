//! Tenant context cache for Aspectus auth middleware.
//!
//! Caches `TenantContext` by token to reduce `/introspect` calls.
//! Uses probabilistic eviction to prevent unbounded memory growth.

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tenant::TenantContext;

struct CacheEntry {
    ctx: TenantContext,
    inserted_at: Instant,
}

/// Token → TenantContext cache with probabilistic eviction.
///
/// Get TTL: 60s (shorter than Aspectus TTL to pick up changes).
/// Cleanup: every 1024th `get()` removes entries older than 300s.
pub struct TenantCache {
    entries: DashMap<String, CacheEntry>,
    counter: AtomicU64,
}

impl TenantCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
            counter: AtomicU64::new(0),
        }
    }

    /// Get cached TenantContext for a token.
    ///
    /// Returns `None` if the token is not cached or the entry is older than 60 seconds.
    /// Every 1024th call triggers a cleanup pass: entries older than 300s are removed.
    pub fn get(&self, token: &str) -> Option<TenantContext> {
        // Probabilistic cleanup every 1024 lookups
        if self.counter.fetch_add(1, Ordering::Relaxed) % 1024 == 0 {
            let now = Instant::now();
            self.entries.retain(|_, v| {
                now.duration_since(v.inserted_at) < Duration::from_secs(300)
            });
        }

        self.entries
            .get(token)
            .filter(|e| e.inserted_at.elapsed() < Duration::from_secs(60))
            .map(|e| e.ctx.clone())
    }

    /// Insert a tenant context keyed by its bearer token.
    pub fn insert(&self, token: String, ctx: TenantContext) {
        self.entries.insert(
            token,
            CacheEntry {
                ctx,
                inserted_at: Instant::now(),
            },
        );
    }

    /// Number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl Default for TenantCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_ctx(tenant_id: &str) -> TenantContext {
        TenantContext {
            tenant_id: tenant_id.to_string(),
            user_id: None,
            scopes: vec![],
            quotas: tenant::TenantQuota {
                max_concurrent_sessions: 10,
                max_tokens_per_day: 1_000_000,
                max_tool_calls_per_minute: 60,
                cpu_time_budget_ms_per_day: 3_600_000,
            },
            cached_at: Instant::now(),
        }
    }

    #[test]
    fn insert_and_get() {
        let cache = TenantCache::new();
        let ctx = make_ctx("test-tenant");
        cache.insert("token-abc".into(), ctx.clone());

        let retrieved = cache.get("token-abc");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().tenant_id, "test-tenant");
    }

    #[test]
    fn get_nonexistent() {
        let cache = TenantCache::new();
        assert!(cache.get("unknown-token").is_none());
    }

    #[test]
    fn insert_and_len() {
        let cache = TenantCache::new();
        assert_eq!(cache.len(), 0);

        cache.insert("t1".into(), make_ctx("tenant-1"));
        cache.insert("t2".into(), make_ctx("tenant-2"));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn get_stale_entry() {
        let cache = TenantCache::new();

        // Insert an entry with an old timestamp
        let old_ctx = TenantContext {
            tenant_id: "old-tenant".into(),
            user_id: None,
            scopes: vec![],
            quotas: tenant::TenantQuota {
                max_concurrent_sessions: 10,
                max_tokens_per_day: 1_000_000,
                max_tool_calls_per_minute: 60,
                cpu_time_budget_ms_per_day: 3_600_000,
            },
            cached_at: Instant::now(),
        };
        let old_entry = CacheEntry {
            ctx: old_ctx,
            inserted_at: Instant::now() - Duration::from_secs(120),
        };
        cache.entries.insert("stale-token".into(), old_entry);

        // Should miss because >60s old
        assert!(cache.get("stale-token").is_none());
    }

    #[test]
    fn insert_same_token_overwrites() {
        let cache = TenantCache::new();
        cache.insert("t".into(), make_ctx("first"));
        cache.insert("t".into(), make_ctx("second"));

        let result = cache.get("t");
        assert_eq!(result.unwrap().tenant_id, "second");
        assert_eq!(cache.len(), 1);
    }
}
