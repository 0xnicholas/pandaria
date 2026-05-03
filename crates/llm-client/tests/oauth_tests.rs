use llm_client::{OAuthToken, is_expired};
use secrecy::{ExposeSecret, SecretString};
use std::time::SystemTime;

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
fn test_oauth_token_debug_no_refresh_token() {
    let token = OAuthToken {
        access_token: SecretString::new("secret_access_token".into()),
        refresh_token: None,
        expires_at: None,
        scopes: vec!["read".to_string()],
    };

    let debug = format!("{:?}", token);
    assert!(!debug.contains("secret_access_token"));
    assert!(debug.contains("scopes"));
}

#[tokio::test]
async fn test_resolve_oauth_key_success() {
    use llm_client::resolve_oauth_key;
    use llm_client::oauth::OAuthProvider;
    use std::sync::Arc;

    struct MockOAuthProvider;

    #[async_trait::async_trait]
    impl OAuthProvider for MockOAuthProvider {
        fn provider_name(&self) -> &str {
            "mock"
        }

        async fn login(&self) -> Result<OAuthToken, std::io::Error> {
            unimplemented!()
        }

        async fn refresh(&self, _token: &OAuthToken) -> Result<OAuthToken, std::io::Error> {
            unimplemented!()
        }

        fn load_token(&self) -> Option<OAuthToken> {
            Some(OAuthToken {
                access_token: SecretString::new("oauth_access_token".into()),
                refresh_token: None,
                expires_at: Some(SystemTime::now() + std::time::Duration::from_secs(3600)),
                scopes: vec![],
            })
        }

        fn save_token(&self, _token: &OAuthToken) -> std::io::Result<()> {
            Ok(())
        }
    }

    let oauth = Some(Arc::new(MockOAuthProvider) as Arc<dyn OAuthProvider>);
    let key = resolve_oauth_key(&oauth).await;
    assert!(key.is_some());
    let secret = key.unwrap();
    assert_eq!(secret.expose_secret(), "oauth_access_token");
}

#[tokio::test]
async fn test_resolve_oauth_key_expired_refresh_success() {
    use llm_client::resolve_oauth_key;
    use llm_client::oauth::OAuthProvider;
    use std::sync::Arc;

    struct MockOAuthProvider;

    #[async_trait::async_trait]
    impl OAuthProvider for MockOAuthProvider {
        fn provider_name(&self) -> &str {
            "mock"
        }

        async fn login(&self) -> Result<OAuthToken, std::io::Error> {
            unimplemented!()
        }

        async fn refresh(&self, _token: &OAuthToken) -> Result<OAuthToken, std::io::Error> {
            Ok(OAuthToken {
                access_token: SecretString::new("refreshed_token".into()),
                refresh_token: None,
                expires_at: Some(SystemTime::now() + std::time::Duration::from_secs(3600)),
                scopes: vec![],
            })
        }

        fn load_token(&self) -> Option<OAuthToken> {
            Some(OAuthToken {
                access_token: SecretString::new("old_token".into()),
                refresh_token: None,
                expires_at: Some(SystemTime::UNIX_EPOCH), // expired
                scopes: vec![],
            })
        }

        fn save_token(&self, _token: &OAuthToken) -> std::io::Result<()> {
            Ok(())
        }
    }

    let oauth = Some(Arc::new(MockOAuthProvider) as Arc<dyn OAuthProvider>);
    let key = resolve_oauth_key(&oauth).await;
    assert!(key.is_some());
    let secret = key.unwrap();
    assert_eq!(secret.expose_secret(), "refreshed_token");
}

#[tokio::test]
async fn test_resolve_oauth_key_no_provider() {
    use llm_client::resolve_oauth_key;

    let oauth: Option<std::sync::Arc<dyn llm_client::oauth::OAuthProvider>> = None;
    let key = resolve_oauth_key(&oauth).await;
    assert!(key.is_none());
}

#[tokio::test]
async fn test_resolve_oauth_key_refresh_failure_falls_back() {
    use llm_client::resolve_oauth_key;
    use llm_client::oauth::OAuthProvider;
    use std::sync::Arc;

    struct FailingOAuthProvider;

    #[async_trait::async_trait]
    impl OAuthProvider for FailingOAuthProvider {
        fn provider_name(&self) -> &str {
            "failing"
        }

        async fn login(&self) -> Result<OAuthToken, std::io::Error> {
            unimplemented!()
        }

        async fn refresh(&self, _token: &OAuthToken) -> Result<OAuthToken, std::io::Error> {
            Err(std::io::Error::new(std::io::ErrorKind::Other, "refresh failed"))
        }

        fn load_token(&self) -> Option<OAuthToken> {
            Some(OAuthToken {
                access_token: SecretString::new("old_token".into()),
                refresh_token: None,
                expires_at: Some(SystemTime::UNIX_EPOCH), // expired
                scopes: vec![],
            })
        }

        fn save_token(&self, _token: &OAuthToken) -> std::io::Result<()> {
            Ok(())
        }
    }

    let oauth = Some(Arc::new(FailingOAuthProvider) as Arc<dyn OAuthProvider>);
    let key = resolve_oauth_key(&oauth).await;
    assert!(key.is_none()); // refresh failed, should return None for fallback
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
