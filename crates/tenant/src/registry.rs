use std::sync::Arc;

use dashmap::DashMap;

use crate::error::TenantError;
use crate::supervisor::TenantSupervisor;
use crate::tenant::Tenant;

/// Global registry of all tenants and their supervisors.
pub struct TenantRegistry {
    tenants: DashMap<String, Arc<TenantSupervisor>>,
}

impl TenantRegistry {
    pub fn new() -> Self {
        Self {
            tenants: DashMap::new(),
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
