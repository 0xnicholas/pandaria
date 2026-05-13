use crate::error::LlmError;
use secrecy::SecretString;
use std::time::SystemTime;

/// OAuth token for LLM provider authentication.
///
/// Uses `secrecy::SecretString` for `access_token` and `refresh_token`
/// so that `Debug` output is automatically redacted.
#[derive(Clone)]
pub struct OAuthToken {
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub expires_at: Option<SystemTime>,
    pub scopes: Vec<String>,
}

impl std::fmt::Debug for OAuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthToken")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// Check whether the token has expired.
pub fn is_expired(token: &OAuthToken) -> bool {
    match token.expires_at {
        Some(expiry) => SystemTime::now() >= expiry,
        None => false,
    }
}

/// OAuth provider abstraction.
///
/// Implementors handle the actual OAuth flow (Browser/Device code/etc.)
/// and token persistence.  This trait is intentionally minimal — the
/// concrete flows live outside llm-client.
#[async_trait::async_trait]
pub trait OAuthProvider: Send + Sync {
    fn provider_name(&self) -> &str;

    /// Acquire a fresh token (e.g. via browser redirect or device code).
    async fn login(&self) -> Result<OAuthToken, LlmError>;

    /// Refresh an existing token.
    async fn refresh(&self, token: &OAuthToken) -> Result<OAuthToken, LlmError>;

    /// Load a previously saved token from disk / keyring / etc.
    fn load_token(&self) -> Option<OAuthToken>;

    /// Persist a token for later reuse.
    fn save_token(&self, token: &OAuthToken) -> Result<(), LlmError>;
}

/// Resolve an API key via OAuth when available.
///
/// 1. If an `OAuthProvider` is configured, try loading the token.
/// 2. If the token is expired, attempt refresh.
/// 3. Return the access token on success.
/// 4. On any failure (missing token, refresh error, etc.), return `None`
///    so the caller can fall back to the next key source.
pub async fn resolve_oauth_key(
    oauth: Option<&std::sync::Arc<dyn OAuthProvider>>,
) -> Option<SecretString> {
    let oauth = oauth?;
    let token = oauth.load_token()?;
    let token = if is_expired(&token) {
        oauth.refresh(&token).await.ok()?
    } else {
        token
    };
    Some(token.access_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_token_debug_redacted() {
        let token = OAuthToken {
            access_token: SecretString::new("secret_access_token".into()),
            refresh_token: Some(SecretString::new("secret_refresh_token".into())),
            expires_at: None,
            scopes: vec![],
        };

        let debug = format!("{:?}", token);
        assert!(!debug.contains("secret_access_token"));
        assert!(!debug.contains("secret_refresh_token"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn test_is_expired_none() {
        let token = OAuthToken {
            access_token: SecretString::new("x".into()),
            refresh_token: None,
            expires_at: None,
            scopes: vec![],
        };
        assert!(!is_expired(&token));
    }

    #[test]
    fn test_is_expired_past() {
        let token = OAuthToken {
            access_token: SecretString::new("x".into()),
            refresh_token: None,
            expires_at: Some(SystemTime::UNIX_EPOCH),
            scopes: vec![],
        };
        assert!(is_expired(&token));
    }

    #[test]
    fn test_is_expired_future() {
        let token = OAuthToken {
            access_token: SecretString::new("x".into()),
            refresh_token: None,
            expires_at: Some(SystemTime::now() + std::time::Duration::from_secs(3600)),
            scopes: vec![],
        };
        assert!(!is_expired(&token));
    }
}
