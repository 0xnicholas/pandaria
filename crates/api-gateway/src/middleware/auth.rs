use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use hmac::{Hmac, Mac};
use secrecy::ExposeSecret;
use sha2::Sha256;
use std::sync::Arc;
use tracing::Instrument;

use crate::error::GatewayError;
use crate::middleware::TenantId;
use crate::server::AppState;

/// Token payload 结构。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenPayload {
    pub tenant_id: String,
    pub iat: u64,
    pub exp: u64,
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

    let tenant_id = payload.tenant_id;

    // 注入 tenant_id
    req.extensions_mut().insert(TenantId(tenant_id.clone()));

    // 在异步 future 上挂载 tracing span，避免跨 await 边界时 span 附着到错误任务
    let span = tracing::info_span!(
        "http_request",
        http.method = %req.method(),
        http.uri = %req.uri(),
        tenant_id = %tenant_id,
    );
    Ok(async move { next.run(req).await }.instrument(span).await)
}

/// HMAC-SHA256 验证 token。
fn verify_token(token_str: &str, secret: &str) -> Option<TokenPayload> {
    use std::time::{SystemTime, UNIX_EPOCH};

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

    // Validate expiration and clock skew
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    if payload.exp <= now {
        return None;
    }
    // iat must not be in the future by more than 5 minutes (clock skew tolerance)
    if payload.iat > now + 300 {
        return None;
    }

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

    #[test]
    fn test_verify_expired_token() {
        let secret = "test-secret-32-chars-long!!!";
        let token = make_token("tenant-1", secret, 1); // expired
        assert!(verify_token(&token, secret).is_none());
    }

    #[test]
    fn test_verify_future_iat_token() {
        let secret = "test-secret-32-chars-long!!!";
        let future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 400; // > 5 min in the future
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
