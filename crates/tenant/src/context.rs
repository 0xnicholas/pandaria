//! Tenant context from Aspectus introspection response.
//!
//! Built by api-gateway auth middleware after successful introspection,
//! injected into request extensions for downstream handlers.

use std::time::Instant;

use crate::error::TenantError;
use crate::tenant::TenantQuota;

/// Tenant context from Aspectus introspection response.
///
/// Contains identity information (tenant_id, user_id, scopes) and
/// Pandaria-specific quota configuration extracted from the `quotas.pandaria`
/// field of the introspection response.
#[derive(Debug, Clone)]
pub struct TenantContext {
    /// The tenant's unique identifier.
    pub tenant_id: String,
    /// The authenticated user (None for service accounts).
    pub user_id: Option<String>,
    /// OAuth2-style scopes (e.g. `["pandaria:session:create"]`).
    pub scopes: Vec<String>,
    /// Pandaria-specific quota limits.
    pub quotas: TenantQuota,
    /// Timestamp when this context was created.
    pub cached_at: Instant,
}

impl TenantContext {
    /// Build from individual fields of an Aspectus [`IntrospectResponse`].
    ///
    /// The caller must have already verified `active == true`.
    /// `quotas_json` should be the value of `response.quotas["pandaria"]`.
    ///
    /// # Errors
    ///
    /// Returns [`TenantError::TenantNotConfigured`] if `quotas_json` is `None`
    /// (i.e. the tenant exists in Aspectus but has no pandaria configuration).
    ///
    /// Returns [`TenantError::InvalidQuotasFormat`] if `quotas_json` exists but
    /// is not a JSON object.
    pub fn from_introspect(
        tenant_id: String,
        user_id: Option<String>,
        scope: Option<String>,
        quotas_json: Option<&serde_json::Value>,
    ) -> Result<Self, TenantError> {
        let quotas_json = quotas_json
            .ok_or_else(|| TenantError::TenantNotConfigured(tenant_id.clone()))?;

        let quotas = TenantQuota::from_aspectus_quotas(quotas_json)?;

        let scopes = scope
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        Ok(Self {
            tenant_id,
            user_id,
            scopes,
            quotas,
            cached_at: Instant::now(),
        })
    }

    /// Check whether the given scope string is present.
    ///
    /// Returns `true` if any scope in this context exactly matches `scope`.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    /// Returns `true` if this context was created more than `ttl` ago.
    pub fn is_stale(&self, ttl: std::time::Duration) -> bool {
        self.cached_at.elapsed() > ttl
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TenantError;

    fn make_quotas(json: serde_json::Value) -> serde_json::Value {
        json
    }

    // ── from_introspect ──────────────────────────────────────

    #[test]
    fn from_introspect_with_valid_quotas() {
        let quotas = make_quotas(serde_json::json!({
            "max_concurrent_sessions": 20,
            "max_tokens_per_day": 2_000_000,
            "max_tool_calls_per_minute": 100,
            "cpu_time_budget_ms_per_day": 7_200_000,
        }));

        let ctx = TenantContext::from_introspect(
            "acme".into(),
            Some("user-1".into()),
            Some("pandaria:session:create pandaria:session:read".into()),
            Some(&quotas),
        )
        .unwrap();

        assert_eq!(ctx.tenant_id, "acme");
        assert_eq!(ctx.user_id.as_deref(), Some("user-1"));
        assert_eq!(ctx.quotas.max_concurrent_sessions, 20);
        assert_eq!(ctx.quotas.max_tokens_per_day, 2_000_000);
    }

    #[test]
    fn from_introspect_missing_pandaria_key() {
        let err = TenantContext::from_introspect(
            "acme".into(),
            None,
            None,
            None, // quotas_json is None
        )
        .unwrap_err();

        assert!(matches!(err, TenantError::TenantNotConfigured(ref id) if id == "acme"));
    }

    #[test]
    fn from_introspect_invalid_quotas_format() {
        let quotas = make_quotas(serde_json::json!("not an object"));

        let err = TenantContext::from_introspect(
            "acme".into(),
            None,
            None,
            Some(&quotas),
        )
        .unwrap_err();

        assert!(matches!(err, TenantError::InvalidQuotasFormat(_)));
    }

    #[test]
    fn from_introspect_no_scope() {
        let quotas = make_quotas(serde_json::json!({}));

        let ctx = TenantContext::from_introspect(
            "acme".into(),
            None,
            None, // scope is None
            Some(&quotas),
        )
        .unwrap();

        assert!(ctx.scopes.is_empty());
    }

    #[test]
    fn from_introspect_no_user() {
        let quotas = make_quotas(serde_json::json!({}));

        let ctx = TenantContext::from_introspect(
            "acme".into(),
            None, // user_id is None (service account)
            Some("pandaria:*".into()),
            Some(&quotas),
        )
        .unwrap();

        assert!(ctx.user_id.is_none());
        assert_eq!(ctx.scopes, vec!["pandaria:*"]);
    }

    #[test]
    fn from_introspect_quotas_partial_fields() {
        // Only some fields provided, rest use defaults
        let quotas = make_quotas(serde_json::json!({
            "max_concurrent_sessions": 5
        }));

        let ctx = TenantContext::from_introspect(
            "acme".into(),
            None,
            None,
            Some(&quotas),
        )
        .unwrap();

        assert_eq!(ctx.quotas.max_concurrent_sessions, 5);
        assert_eq!(ctx.quotas.max_tokens_per_day, 1_000_000); // default
    }

    // ── has_scope ────────────────────────────────────────────

    #[test]
    fn has_scope_positive() {
        let ctx = TenantContext {
            tenant_id: "acme".into(),
            user_id: None,
            scopes: vec!["pandaria:session:create".into(), "pandaria:session:read".into()],
            quotas: TenantQuota::default(),
            cached_at: std::time::Instant::now(),
        };

        assert!(ctx.has_scope("pandaria:session:create"));
        assert!(ctx.has_scope("pandaria:session:read"));
    }

    #[test]
    fn has_scope_negative() {
        let ctx = TenantContext {
            tenant_id: "acme".into(),
            user_id: None,
            scopes: vec!["pandaria:session:read".into()],
            quotas: TenantQuota::default(),
            cached_at: std::time::Instant::now(),
        };

        assert!(!ctx.has_scope("pandaria:session:create"));
        assert!(!ctx.has_scope("tavern:workflow:run"));
    }

    #[test]
    fn has_scope_empty_scopes() {
        let ctx = TenantContext {
            tenant_id: "acme".into(),
            user_id: None,
            scopes: vec![],
            quotas: TenantQuota::default(),
            cached_at: std::time::Instant::now(),
        };

        assert!(!ctx.has_scope("anything"));
    }

    // ── is_stale ─────────────────────────────────────────────

    #[test]
    fn is_stale_fresh() {
        let ctx = TenantContext {
            tenant_id: "acme".into(),
            user_id: None,
            scopes: vec![],
            quotas: TenantQuota::default(),
            cached_at: std::time::Instant::now(),
        };

        // Not stale with a large TTL
        assert!(!ctx.is_stale(std::time::Duration::from_secs(3600)));
    }

    #[test]
    fn is_stale_expired() {
        let ctx = TenantContext {
            tenant_id: "acme".into(),
            user_id: None,
            scopes: vec![],
            quotas: TenantQuota::default(),
            // cached_at is far in the past
            cached_at: std::time::Instant::now() - std::time::Duration::from_secs(120),
        };

        // Stale with a short TTL
        assert!(ctx.is_stale(std::time::Duration::from_secs(60)));
    }

    #[test]
    fn is_stale_exactly_at_boundary() {
        // Elapsed is exactly 60s, TTL is 60s → not stale (elapsed > ttl is false)
        let ctx = TenantContext {
            tenant_id: "acme".into(),
            user_id: None,
            scopes: vec![],
            quotas: TenantQuota::default(),
            cached_at: std::time::Instant::now() - std::time::Duration::from_secs(60),
        };

        // Due to test execution time, elapsed may be slightly > 60s. Use a
        // slightly larger TTL to avoid flakiness.
        let is_stale = ctx.is_stale(std::time::Duration::from_secs(61));
        assert!(!is_stale);
    }
}
