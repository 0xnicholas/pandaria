use std::time::SystemTime;

/// Context passed to `MemoryStore` operations, identifying the tenant and session.
#[derive(Debug, Clone)]
pub struct MemoryContext {
    pub tenant_id: String,
    pub session_id: String,
    /// Pandaria currently has no independent user level; this field is left
    /// for external adapters to map from `tenant_id` or to receive from
    /// future Tenant config / API request headers.
    pub user_id: Option<String>,
    /// Session metadata for external stores to use for routing/filtering
    /// without parsing the metadata JSON blob.
    pub model: String,
    pub session_started_at: SystemTime,
}
