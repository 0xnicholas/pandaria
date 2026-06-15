use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;
use tracing::Instrument;

use crate::error::GatewayError;
use crate::middleware::TenantId;
use crate::server::AppState;

/// Extract Bearer token from Authorization header.
fn extract_bearer_token(req: &Request) -> Result<&str, GatewayError> {
    req.headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(GatewayError::Unauthorized)
}

// ─── HMAC Auth (active when aspectus-auth feature is OFF) ───

#[cfg(not(feature = "aspectus-auth"))]
use hmac::{Hmac, Mac};
#[cfg(not(feature = "aspectus-auth"))]
use secrecy::ExposeSecret;
#[cfg(not(feature = "aspectus-auth"))]
use sha2::Sha256;

#[cfg(not(feature = "aspectus-auth"))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenPayload {
    pub tenant_id: String,
    pub iat: u64,
    pub exp: u64,
}

/// HMAC-SHA256 自签名 token 认证（legacy）。
#[cfg(not(feature = "aspectus-auth"))]
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, GatewayError> {
    if req.uri().path() == "/healthz" {
        return Ok(next.run(req).await);
    }

    let token_str = extract_bearer_token(&req)?;
    let payload = verify_token(token_str, state.config.auth_secret.expose_secret())
        .ok_or(GatewayError::Unauthorized)?;

    let tenant_id = payload.tenant_id;
    req.extensions_mut().insert(TenantId(tenant_id.clone()));

    let span = tracing::info_span!(
        "http_request",
        http.method = %req.method(),
        http.uri = %req.uri(),
        tenant_id = %tenant_id,
    );
    Ok(async move { next.run(req).await }.instrument(span).await)
}

#[cfg(not(feature = "aspectus-auth"))]
fn verify_token(token_str: &str, secret: &str) -> Option<TokenPayload> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let parts: Vec<&str> = token_str.split('.').collect();
    if parts.len() != 2 {
        return None;
    }

    let payload_bytes = base64_decode_urlsafe(parts[0])?;
    let signature = base64_decode_urlsafe(parts[1])?;

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(&payload_bytes);
    mac.verify_slice(&signature).ok()?;

    let payload: TokenPayload = serde_json::from_slice(&payload_bytes).ok()?;

    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    if payload.exp <= now {
        return None;
    }
    if payload.iat > now + 300 {
        return None;
    }

    Some(payload)
}

#[cfg(not(feature = "aspectus-auth"))]
fn base64_decode_urlsafe(input: &str) -> Option<Vec<u8>> {
    let padding = (4 - input.len() % 4) % 4;
    let padded = format!("{}{}", input, "=".repeat(padding));
    base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE, padded).ok()
}

// ─── Aspectus Auth (active when aspectus-auth feature is ON) ───

#[cfg(feature = "aspectus-auth")]
use std::time::Duration;

#[cfg(feature = "aspectus-auth")]
use tenant::TenantContext;

/// Aspectus Token Introspection 认证（RFC 7662）。
#[cfg(feature = "aspectus-auth")]
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

    // Cache and inject
    state.tenant_cache.insert(token_str.to_string(), ctx.clone());
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

#[cfg(feature = "aspectus-auth")]
async fn introspect_with_retry(
    client: &aspectus_client::AspectusClient,
    token: &str,
) -> Result<aspectus_core::introspect::IntrospectResponse, aspectus_client::ClientError> {
    let mut attempts = 0;
    loop {
        match client.introspect(token).await {
            Ok(resp) => return Ok(resp),
            Err(e) if attempts < 2 => {
                attempts += 1;
                let delay = Duration::from_millis(100 * 2u64.pow(attempts - 1));
                tokio::time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "aspectus-auth"))]
    fn make_token(tenant_id: &str, secret: &str, exp: u64) -> String {
        let payload = TokenPayload {
            tenant_id: tenant_id.into(),
            iat: 1714608000,
            exp,
        };
        let payload_json = serde_json::to_vec(&payload).unwrap();
        let payload_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            &payload_json,
        );

        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&payload_json);
        let signature = mac.finalize().into_bytes();
        let sig_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            &signature,
        );

        format!("{}.{}", payload_b64, sig_b64)
    }

    #[cfg(not(feature = "aspectus-auth"))]
    #[test]
    fn test_verify_valid_token() {
        let secret = "test-secret-32-chars-long!!!";
        let future_exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let token = make_token("tenant-1", secret, future_exp);
        let payload = verify_token(&token, secret).unwrap();
        assert_eq!(payload.tenant_id, "tenant-1");
        assert_eq!(payload.iat, 1714608000);
        assert_eq!(payload.exp, future_exp);
    }

    #[cfg(not(feature = "aspectus-auth"))]
    #[test]
    fn test_verify_expired_token() {
        let secret = "test-secret-32-chars-long!!!";
        let token = make_token("tenant-1", secret, 1); // expired
        assert!(verify_token(&token, secret).is_none());
    }

    #[cfg(not(feature = "aspectus-auth"))]
    #[test]
    fn test_verify_future_iat_token() {
        let secret = "test-secret-32-chars-long!!!";
        let future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 400;
        let payload = TokenPayload {
            tenant_id: "tenant-1".into(),
            iat: future,
            exp: future + 3600,
        };
        let payload_json = serde_json::to_vec(&payload).unwrap();
        let payload_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            &payload_json,
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&payload_json);
        let signature = mac.finalize().into_bytes();
        let sig_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            &signature,
        );
        let token = format!("{}.{}", payload_b64, sig_b64);
        assert!(verify_token(&token, secret).is_none());
    }

    #[cfg(not(feature = "aspectus-auth"))]
    #[test]
    fn test_verify_invalid_signature() {
        let secret = "test-secret-32-chars-long!!!";
        let future_exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let token = make_token("tenant-1", secret, future_exp);
        let result = verify_token(&token, "wrong-secret-32-chars-long!");
        assert!(result.is_none());
    }

    #[cfg(not(feature = "aspectus-auth"))]
    #[test]
    fn test_verify_malformed_token() {
        let result = verify_token("not-a-token", "secret");
        assert!(result.is_none());
    }

    #[cfg(not(feature = "aspectus-auth"))]
    #[test]
    fn test_base64_decode_urlsafe() {
        let encoded = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            b"hello",
        );
        let decoded = base64_decode_urlsafe(&encoded).unwrap();
        assert_eq!(decoded, b"hello");
    }
}
