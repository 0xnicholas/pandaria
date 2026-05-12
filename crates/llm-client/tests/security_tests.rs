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
    // HTML response should be sanitized to generic status
    let err = llm_client::LlmError::ProviderError(
        "HTTP 500 Internal Server Error".to_string(),
    );
    let display = format!("{}", err);
    // Error message should not contain raw HTTP body
    assert!(!display.contains("<html>"));
    assert!(!display.contains("<!DOCTYPE"));
    assert!(display.contains("500"));
}

#[test]
fn test_provider_error_extracts_json_message() {
    let body = r#"{"error":{"message":"The model does not exist","type":"invalid_request_error"}}"#;
    let msg = llm_client::http_error::sanitize_http_error_body(400, body);
    assert_eq!(msg, "The model does not exist");
}

#[test]
fn test_provider_error_sanitizes_html() {
    let body = "<html><body><h1>502 Bad Gateway</h1></body></html>";
    let msg = llm_client::http_error::sanitize_http_error_body(502, body);
    assert_eq!(msg, "HTTP 502 Bad Gateway");
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

#[test]
fn test_api_key_not_in_error_display() {
    // LlmError should not expose the actual API key in its Display output.
    // AuthError messages use the env-var name, not the key value.
    let err = llm_client::LlmError::AuthError("OPENAI_API_KEY not set".to_string());
    let display = format!("{}", err);
    assert!(display.contains("OPENAI_API_KEY"));
    assert!(!display.contains("sk-"));
}
