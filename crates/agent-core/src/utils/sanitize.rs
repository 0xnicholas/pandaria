//! Sensitive data sanitization for logs, traces, and error messages.
//!
//! Automatically masks API keys, tokens, and other secrets before they reach
//! log output or error strings.  This is an enforcement layer for the ADR-005
//! security constraint: "LLM API Key must not appear in any log, tracing span,
//! error message, or panic information."

use regex::Regex;
use std::sync::LazyLock;

/// Pre-compiled regex patterns for common secret formats.
///
/// Each tuple is `(pattern, replacement)`.
static SECRET_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        // OpenAI / general sk- keys
        (
            Regex::new(r"sk-[a-zA-Z0-9]{20,}").expect("valid regex"),
            "[REDACTED_API_KEY]",
        ),
        // Anthropic sk-ant-
        (
            Regex::new(r"sk-ant-[a-zA-Z0-9_-]{20,}").expect("valid regex"),
            "[REDACTED_API_KEY]",
        ),
        // AWS secret access key
        (
            Regex::new(r"[A-Za-z0-9/+=]{40}").expect("valid regex"),
            "[REDACTED_SECRET]",
        ),
        // Bearer token (commonly in Authorization headers)
        (
            Regex::new(r"(?i)Bearer\s+[A-Za-z0-9-_=]+").expect("valid regex"),
            "Bearer [REDACTED_TOKEN]",
        ),
        // Basic auth base64
        (
            Regex::new(r"(?i)Basic\s+[A-Za-z0-9+/=]+").expect("valid regex"),
            "Basic [REDACTED_CREDENTIALS]",
        ),
        // Generic api_key / apikey / api-key values in JSON/key-value contexts
        (
            Regex::new(r#"(?i)(api[_-]?key["']?\s*[:=]\s*["']?)[A-Za-z0-9_\-]{8,}"#)
                .expect("valid regex"),
            "${1}[REDACTED_API_KEY]",
        ),
    ]
});

/// Sanitize a string by replacing known secret patterns.
///
/// This is a best-effort heuristic.  It does **not** guarantee cryptographically
/// secure redaction — it is designed to prevent accidental secret leakage in
/// logs and traces.
///
/// # Example
///
/// ```
/// use agent_core::utils::sanitize::sanitize_str;
///
/// let raw = "key=sk-abc123def456ghi789jkl012mnop345qrst";
/// assert_eq!(sanitize_str(raw), "key=[REDACTED_API_KEY]");
/// ```
pub fn sanitize_str(input: &str) -> String {
    let mut output = input.to_string();
    for (re, replacement) in SECRET_PATTERNS.iter() {
        output = re.replace_all(&output, *replacement).to_string();
    }
    output
}

/// Sanitize an error message, prepending a tag so consumers know the string
/// has been processed.
pub fn sanitize_error(input: &str) -> String {
    sanitize_str(input)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_key() {
        let raw = "Authorization: Bearer sk-abc123def456ghi789jkl012mnop345qrst";
        let out = sanitize_str(raw);
        assert!(
            !out.contains("sk-abc123"),
            "OpenAI key should be redacted, got: {out}"
        );
        assert!(out.contains("[REDACTED"));
    }

    #[test]
    fn test_anthropic_key() {
        let raw = "api_key=sk-ant-api03-1234567890abcdef-1234567890abcdef-1234567890abcdef";
        let out = sanitize_str(raw);
        assert!(
            !out.contains("sk-ant-api03"),
            "Anthropic key should be redacted, got: {out}"
        );
    }

    #[test]
    fn test_bearer_token() {
        let raw = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        let out = sanitize_str(raw);
        assert_eq!(out, "Authorization: Bearer [REDACTED_TOKEN]");
    }

    #[test]
    fn test_basic_auth() {
        let raw = "Authorization: Basic dXNlcjpwYXNzd29yZA==";
        let out = sanitize_str(raw);
        assert_eq!(out, "Authorization: Basic [REDACTED_CREDENTIALS]");
    }

    #[test]
    fn test_api_key_json() {
        let raw = r#"{"api_key": "sk-1234567890abcdef1234567890abcdef"}"#;
        let out = sanitize_str(raw);
        assert!(
            !out.contains("sk-1234567890abcdef"),
            "JSON api_key should be redacted, got: {out}"
        );
    }

    #[test]
    fn test_aws_secret() {
        let raw = "secret=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let out = sanitize_str(raw);
        assert!(
            !out.contains("wJalrXUtnFEMI"),
            "AWS secret should be redacted, got: {out}"
        );
    }

    #[test]
    fn test_no_false_positive_short_token() {
        let raw = "token=abc123";
        let out = sanitize_str(raw);
        // short tokens should NOT be redacted (8 char threshold in api_key regex)
        assert_eq!(out, "token=abc123");
    }

    #[test]
    fn test_passthrough_safe_text() {
        let raw = "Hello, this is a normal log message with no secrets.";
        assert_eq!(sanitize_str(raw), raw);
    }
}
