/// Context passed to `MemoryStore` operations, identifying the tenant and session.
#[derive(Debug, Clone)]
pub struct MemoryContext {
    pub tenant_id: String,
    pub session_id: String,
    /// Pandaria currently has no independent user level; this field is left
    /// for external adapters to map from `tenant_id` or to receive from
    /// future Tenant config / API request headers.
    pub user_id: Option<String>,
}

/// A single memory fact.  External systems decide how to index / vectorize
/// `content`.
#[derive(Debug, Clone)]
pub struct MemoryFact {
    pub id: Option<String>,
    pub content: String,
    pub category: Option<String>,
    pub importance: Option<u8>,
    pub metadata: serde_json::Value,
}

/// Query used to retrieve memories from an external store.
#[derive(Debug, Clone)]
pub struct MemoryQuery {
    pub text: String,
    pub limit: usize,
    pub session_only: bool,
}
