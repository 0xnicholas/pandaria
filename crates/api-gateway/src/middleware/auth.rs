use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use hmac::{Hmac, Mac};
use secrecy::ExposeSecret;
use sha2::Sha256;
use std::sync::Arc;

use crate::error::GatewayError;
use crate::middleware::TenantId;
use crate::server::AppState;

/// Token payload 结构。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenPayload {
    pub tenant_id: String,
    pub iat: u64,
}

/// 从 Authorization header 提取 tenant_id，注入 request extensions。
/// 认证失败返回 `GatewayError::Unauthorized`。
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, GatewayError> {
    // 跳过 /healthz
    if req.uri().path() == "/healthz" {
        return Ok(next.run(req).await);
    }

    // 提取 Authorization header
    let header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let token_str = match header {
        Some(t) => t,
        None => return Err(GatewayError::Unauthorized),
    };

    // 验证签名
    let payload = match verify_token(token_str, state.config.auth_secret.expose_secret()) {
        Some(p) => p,
        None => return Err(GatewayError::Unauthorized),
    };

    // 创建带 tenant_id 的 tracing span
    let span = tracing::info_span!(
        "http_request",
        http.method = %req.method(),
        http.uri = %req.uri(),
        tenant_id = %payload.tenant_id,
    );
    let _enter = span.enter();

    // 注入 tenant_id
    req.extensions_mut().insert(TenantId(payload.tenant_id));

    Ok(next.run(req).await)
}

/// HMAC-SHA256 验证 token。
fn verify_token(token_str: &str, secret: &str) -> Option<TokenPayload> {
    let parts: Vec<&str> = token_str.split('.').collect();
    if parts.len() != 2 {
        return None;
    }

    let payload_b64 = parts[0];
    let signature_b64 = parts[1];

    let payload_bytes = base64_decode_urlsafe(payload_b64)?;
    let signature = base64_decode_urlsafe(signature_b64)?;

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(&payload_bytes);
    mac.verify_slice(&signature).ok()?;

    let payload: TokenPayload = serde_json::from_slice(&payload_bytes).ok()?;
    Some(payload)
}

fn base64_decode_urlsafe(input: &str) -> Option<Vec<u8>> {
    let padding = (4 - input.len() % 4) % 4;
    let padded = format!("{}{}", input, "=".repeat(padding));
    base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE, padded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_token(tenant_id: &str, secret: &str) -> String {
        let payload = TokenPayload {
            tenant_id: tenant_id.into(),
            iat: 1714608000,
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

    #[test]
    fn test_verify_valid_token() {
        let secret = "test-secret-32-chars-long!!!";
        let token = make_token("tenant-1", secret);
        let payload = verify_token(&token, secret).unwrap();
        assert_eq!(payload.tenant_id, "tenant-1");
        assert_eq!(payload.iat, 1714608000);
    }

    #[test]
    fn test_verify_invalid_signature() {
        let secret = "test-secret-32-chars-long!!!";
        let token = make_token("tenant-1", secret);
        let result = verify_token(&token, "wrong-secret-32-chars-long!");
        assert!(result.is_none());
    }

    #[test]
    fn test_verify_malformed_token() {
        let result = verify_token("not-a-token", "secret");
        assert!(result.is_none());
    }

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
