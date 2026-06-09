/// Generate a development HMAC token for local testing.
///
/// This matches the token generation logic in `api-gateway/src/main.rs`
/// and `api-gateway/tests/common/mod.rs`. When the api-gateway server
/// is started without `PANDARIA_AUTH_SECRET`, it falls back to a
/// hard-coded test secret. This module lets the TUI automatically
/// generate a valid token for that local server so developers do not
/// need to manually configure one.
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Known fallback secrets used by the api-gateway server in dev mode.
///
/// When `PANDARIA_DEV_MODE=1` is set, the api-gateway server uses
/// `"pandaria-dev-secret-32chars-long!"` as its auth secret.
/// The TUI tries these secrets in order when auto-authenticating to a
/// local development server.
const DEV_SECRETS: &[&str] = &[
    // Dev-mode secret used when PANDARIA_DEV_MODE=1 is set.
    "pandaria-dev-secret-32chars-long!",
    // Legacy test secret used in integration tests and default config.
    "test-secret-32-chars-long!!!",
];

/// Generate a token signed with the given secret.
fn generate_token(secret: &str, tenant_id: &str) -> String {
    let payload = serde_json::json!({
        "tenant_id": tenant_id,
        "iat": 1714608000u64,
    });
    let payload_json = serde_json::to_vec(&payload).expect("json encode");
    let payload_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        &payload_json,
    );

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(&payload_json);
    let signature = mac.finalize().into_bytes();
    let sig_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        signature,
    );

    format!("{}.{}", payload_b64, sig_b64)
}

/// Return true if the URL points to a local development server.
fn is_local_dev(url: &str) -> bool {
    url.contains("localhost") || url.contains("127.0.0.1")
}

/// Attempt to produce a valid dev token for the given server URL.
///
/// Returns `None` when the URL is not a local dev server.
pub fn try_dev_token(url: &str) -> Option<String> {
    if !is_local_dev(url) {
        return None;
    }
    // Use the first known secret to generate a token. The TUI startup
    // logic will try each secret if the connection fails.
    Some(generate_token(DEV_SECRETS[0], "test-tenant"))
}

/// Generate tokens for all known dev secrets so the caller can try
/// them one by one.
pub fn dev_tokens(url: &str) -> Vec<String> {
    if !is_local_dev(url) {
        return Vec::new();
    }
    DEV_SECRETS
        .iter()
        .map(|secret| generate_token(secret, "test-tenant"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_local_dev() {
        assert!(is_local_dev("http://localhost:8080"));
        assert!(is_local_dev("http://127.0.0.1:8080"));
        assert!(!is_local_dev("https://api.example.com"));
    }

    #[test]
    fn test_generate_token_format() {
        let token = generate_token("test-secret-32-chars-long!!!", "test-tenant");
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn test_try_dev_token_for_localhost() {
        let token = try_dev_token("http://localhost:8080");
        assert!(token.is_some());
    }

    #[test]
    fn test_try_dev_token_for_remote() {
        let token = try_dev_token("https://api.example.com");
        assert!(token.is_none());
    }
}
