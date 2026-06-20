use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;
use std::time::Duration;
use tracing::Instrument;

use crate::error::GatewayError;
use crate::middleware::TenantId;
use crate::server::AppState;
use tenant::TenantContext;

/// Extract Bearer token from Authorization header.
fn extract_bearer_token(req: &Request) -> Result<&str, GatewayError> {
    req.headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(GatewayError::Unauthorized)
}

/// Aspectus Token Introspection 认证（RFC 7662）。
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, GatewayError> {
    if req.uri().path() == "/healthz" {
        return Ok(next.run(req).await);
    }

    let token_str = extract_bearer_token(&req)?;

    // Check cache first (reduces Aspectus calls)
    if let Some(ctx) = state.tenant_cache.get(token_str) {
        req.extensions_mut().insert(TenantId(ctx.tenant_id.clone()));
        req.extensions_mut().insert(ctx.clone());
        let span = tracing::info_span!(
            "http_request",
            http.method = %req.method(),
            http.uri = %req.uri(),
            tenant_id = %ctx.tenant_id,
        );
        return Ok(async move { next.run(req).await }.instrument(span).await);
    }

    // Introspect with retry
    let introspect = introspect_with_retry(&state.aspectus, token_str)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "aspectus introspection failed");
            GatewayError::ServiceUnavailable
        })?;

    if !introspect.active {
        return Err(GatewayError::Unauthorized);
    }

    let tenant_id = introspect
        .tenant_id
        .ok_or(GatewayError::Unauthorized)?;

    let ctx = TenantContext::from_introspect(
        tenant_id,
        introspect.user_id,
        introspect.scope,
        introspect.quotas.as_ref().and_then(|q| q.get("pandaria")),
    )
    .map_err(|e| match &e {
        tenant::TenantError::TenantNotConfigured(_) => {
            GatewayError::Forbidden("tenant not configured for pandaria".into())
        }
        _ => GatewayError::Internal(e.to_string()),
    })?;

    // Cache and inject. Also ensure supervisor exists in registry for all routes.
    state.tenant_cache.insert(token_str.to_string(), ctx.clone());
    // Ensure tenant supervisor exists (required by get_quota, batch_create, etc.)
    if let Err(e) = state.registry.resolve_or_insert(&ctx) {
        tracing::error!(error = %e, tenant_id = %ctx.tenant_id, "failed to resolve tenant");
        return Err(GatewayError::Internal(e.to_string()));
    }
    req.extensions_mut().insert(TenantId(ctx.tenant_id.clone()));
    req.extensions_mut().insert(ctx.clone());

    let span = tracing::info_span!(
        "http_request",
        http.method = %req.method(),
        http.uri = %req.uri(),
        tenant_id = %ctx.tenant_id,
    );
    Ok(async move { next.run(req).await }.instrument(span).await)
}

/// Call Aspectus introspection with exponential backoff retry (max 2 retries).
async fn introspect_with_retry(
    client: &aspectus_client::AspectusClient,
    token: &str,
) -> Result<aspectus_core::introspect::IntrospectResponse, aspectus_client::ClientError> {
    let mut attempts = 0;
    loop {
        match client.introspect(token).await {
            Ok(resp) => return Ok(resp),
            Err(_e) if attempts < 2 => {
                attempts += 1;
                let delay = Duration::from_millis(100 * 2u64.pow(attempts - 1));
                tokio::time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}
