use llm_client::StreamOptions;
use secrecy::SecretString;

#[test]
fn test_secret_string_debug_redacted() {
    let secret = SecretString::new("sk-abc123-def456".into());
    let debug = format!("{:?}", secret);
    assert!(!debug.contains("sk-abc123"));
}

#[test]
fn test_provider_error_no_raw_body() {
    let err = llm_client::LlmError::ProviderError(
        "HTTP 500: internal server error".to_string(),
    );
    let display = format!("{}", err);
    // Error message should not contain raw HTTP body
    assert!(!display.contains("<html>"));
    assert!(!display.contains("<!DOCTYPE"));
    assert!(display.contains("internal server error"));
}

#[test]
fn test_stream_options_api_key_redacted() {
    let options = StreamOptions {
        api_key: Some(SecretString::new("sk-secret-key".into())),
        ..Default::default()
    };
    let debug = format!("{:?}", options);
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("sk-secret-key"));
}
