/// Sanitize an HTTP error response body for safe inclusion in LlmError messages.
///
/// Attempts to extract a safe `message` field from standard provider error JSON schemas.
/// Falls back to a generic HTTP status description if the body is HTML, binary,
/// or does not match a known schema.
///
/// Known schemas (checked in order):
/// 1. OpenAI / Mistral: `{ "error": { "message": "...", ... } }`
/// 2. Anthropic: `{ "type": "error", "error": { "type": "...", "message": "..." } }`
/// 3. Google: `{ "error": { "code": N, "message": "...", "status": "..." } }`
/// 4. Generic: any JSON with a top-level or nested `message` string field
pub fn sanitize_http_error_body(status: u16, body: &str) -> String {
    // Skip HTML and obvious non-JSON responses quickly.
    let trimmed = body.trim();
    if trimmed.starts_with('<') || trimmed.is_empty() {
        return format_http_status(status);
    }

    // Try JSON extraction.
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
        // 1. OpenAI / Mistral / Google: error.message
        if let Some(msg) = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return sanitize_message(msg);
        }

        // 2. Generic fallback: any message field anywhere in the JSON
        if let Some(msg) = find_message_field(&json) {
            return sanitize_message(msg);
        }
    }

    format_http_status(status)
}

fn format_http_status(status: u16) -> String {
    match reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
    {
        Some(reason) => format!("HTTP {} {}", status, reason),
        None => format!("HTTP {}", status),
    }
}

fn sanitize_message(msg: &str) -> String {
    // Trim and truncate to a reasonable length to avoid massive error messages.
    let trimmed = msg.trim();
    const MAX_LEN: usize = 256;
    if trimmed.len() > MAX_LEN {
        format!("{}...", &trimmed[..MAX_LEN])
    } else {
        trimmed.to_string()
    }
}

fn find_message_field(value: &serde_json::Value) -> Option<&str> {
    match value {
        serde_json::Value::Object(map) => {
            // Direct message field
            if let Some(msg) = map.get("message").and_then(|m| m.as_str()) {
                return Some(msg);
            }
            // Recurse into nested objects
            for v in map.values() {
                if let Some(msg) = find_message_field(v) {
                    return Some(msg);
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_body_returns_status() {
        let body = "<html><body><h1>502 Bad Gateway</h1></body></html>";
        let msg = sanitize_http_error_body(502, body);
        assert_eq!(msg, "HTTP 502 Bad Gateway");
    }

    #[test]
    fn test_empty_body_returns_status() {
        let msg = sanitize_http_error_body(500, "");
        assert_eq!(msg, "HTTP 500 Internal Server Error");
    }

    #[test]
    fn test_openai_error_schema() {
        let body =
            r#"{"error":{"message":"The model does not exist","type":"invalid_request_error"}}"#;
        let msg = sanitize_http_error_body(400, body);
        assert_eq!(msg, "The model does not exist");
    }

    #[test]
    fn test_anthropic_error_schema() {
        let body = r#"{"type":"error","error":{"type":"authentication_error","message":"Invalid API key"}}"#;
        let msg = sanitize_http_error_body(401, body);
        assert_eq!(msg, "Invalid API key");
    }

    #[test]
    fn test_google_error_schema() {
        let body =
            r#"{"error":{"code":400,"message":"API key expired","status":"INVALID_ARGUMENT"}}"#;
        let msg = sanitize_http_error_body(400, body);
        assert_eq!(msg, "API key expired");
    }

    #[test]
    fn test_generic_json_with_message() {
        let body = r#"{"message":"Something went wrong"}"#;
        let msg = sanitize_http_error_body(503, body);
        assert_eq!(msg, "Something went wrong");
    }

    #[test]
    fn test_message_truncation() {
        let long_msg = "a".repeat(300);
        let body = format!(r#"{{"error":{{"message":"{}"}}}}"#, long_msg);
        let msg = sanitize_http_error_body(400, &body);
        assert!(msg.ends_with("..."));
        assert_eq!(msg.len(), 259); // 256 + "..."
    }

    #[test]
    fn test_unknown_json_returns_status() {
        let body = r#"{"foo":"bar"}"#;
        let msg = sanitize_http_error_body(403, body);
        assert_eq!(msg, "HTTP 403 Forbidden");
    }

    #[test]
    fn test_invalid_json_returns_status() {
        let body = "not json at all";
        let msg = sanitize_http_error_body(400, body);
        assert_eq!(msg, "HTTP 400 Bad Request");
    }
}
