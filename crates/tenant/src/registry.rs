use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;

use crate::context::TenantContext;
use crate::error::TenantError;
use crate::supervisor::TenantSupervisor;
use crate::tenant::Tenant;

/// Global registry of all tenants and their supervisors.
///
/// Supports both legacy registration (`register`) and Aspectus-driven
/// resolution (`resolve_or_insert` with TTL cache).
pub struct TenantRegistry {
    tenants: DashMap<String, Arc<TenantSupervisor>>,
    cache_ttl: Duration,
}

impl TenantRegistry {
    /// Create a new registry with default TTL (300s).
    pub fn new() -> Self {
        Self {
            tenants: DashMap::new(),
            cache_ttl: Duration::from_secs(300),
        }
    }

    /// Create a new registry with a custom cache TTL.
    pub fn with_ttl(cache_ttl: Duration) -> Self {
        Self {
            tenants: DashMap::new(),
            cache_ttl,
        }
    }

    /// Register a new tenant. Fails if tenant_id already exists.
    pub fn register(&self, tenant: Tenant) -> Result<(), TenantError> {
        let id = tenant.id.clone();
        let supervisor = Arc::new(TenantSupervisor::new(tenant));
        match self.tenants.entry(id.clone()) {
            dashmap::mapref::entry::Entry::Occupied(_) => Err(TenantError::TenantAlreadyExists(id)),
            dashmap::mapref::entry::Entry::Vacant(e) => {
                e.insert(supervisor);
                Ok(())
            }
        }
    }

    /// Resolve or create a TenantSupervisor from TenantContext.
    /// Returns cached supervisor if present and not stale (within cache_ttl).
    pub fn resolve_or_insert(
        &self,
        ctx: &TenantContext,
    ) -> Result<Arc<TenantSupervisor>, TenantError> {
        if let Some(existing) = self.tenants.get(&ctx.tenant_id)
            && !existing.is_stale(self.cache_ttl) {
                return Ok(existing.clone());
            }
        let supervisor = Arc::new(TenantSupervisor::from_context(ctx));
        self.tenants.insert(ctx.tenant_id.clone(), supervisor.clone());
        Ok(supervisor)
    }

    /// Force refresh a tenant's supervisor from new context.
    pub fn refresh(
        &self,
        ctx: &TenantContext,
    ) -> Result<Arc<TenantSupervisor>, TenantError> {
        let supervisor = Arc::new(TenantSupervisor::from_context(ctx));
        self.tenants.insert(ctx.tenant_id.clone(), supervisor.clone());
        Ok(supervisor)
    }

    /// Evict a tenant from the cache.
    pub fn evict(&self, tenant_id: &str) -> Option<Arc<TenantSupervisor>> {
        self.tenants.remove(tenant_id).map(|(_, v)| v)
    }

    /// Look up a tenant's supervisor.
    pub fn get(&self, tenant_id: &str) -> Option<Arc<TenantSupervisor>> {
        self.tenants.get(tenant_id).map(|entry| entry.clone())
    }

    /// Unregister a tenant, returning its supervisor if it existed.
    pub fn unregister(&self, tenant_id: &str) -> Option<Arc<TenantSupervisor>> {
        self.tenants.remove(tenant_id).map(|(_, v)| v)
    }

    /// Check whether a tenant is registered.
    pub fn contains(&self, tenant_id: &str) -> bool {
        self.tenants.contains_key(tenant_id)
    }

    /// Returns active session counts keyed by tenant_id.
    pub fn active_session_counts(&self) -> std::collections::HashMap<String, usize> {
        self.tenants
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().active_session_count()))
            .collect()
    }

    /// Number of registered tenants.
    pub fn len(&self) -> usize {
        self.tenants.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tenants.is_empty()
    }
}

impl Default for TenantRegistry {
    fn default() -> Self {
        Self::new()
    }
}
