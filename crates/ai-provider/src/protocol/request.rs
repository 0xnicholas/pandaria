use std::collections::HashMap;

use crate::error::LlmError;
use crate::models::Model;
use crate::provider::StreamOptions;

/// Unified HTTP request builder for LLM provider APIs.
///
/// Encapsulates the common flow shared by all providers:
/// 1. Build JSON body (caller-supplied)
/// 2. Apply `on_payload` hook
/// 3. Set headers (auth + custom)
/// 4. Send POST request
/// 5. Apply `on_response` hook
/// 6. Validate status code, sanitize error body
/// 7. Return raw `reqwest::Response` for SSE parsing
pub struct RequestBuilder {
    client: reqwest::Client,
    url: String,
    body: serde_json::Value,
    headers: HashMap<String, String>,
    options: StreamOptions,
    fallback_model: Model,
}

impl RequestBuilder {
    pub fn new(
        client: reqwest::Client,
        url: String,
        fallback_model: Model,
        options: StreamOptions,
    ) -> Self {
        Self {
            client,
            url,
            body: serde_json::Value::Null,
            headers: HashMap::new(),
            options,
            fallback_model,
        }
    }

    /// Set the JSON request body. Call this before any hook-dependent ops.
    pub fn body(mut self, body: serde_json::Value) -> Self {
        self.body = body;
        self
    }

    /// Add a single header.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Add multiple headers at once.
    pub fn headers(mut self, headers: HashMap<String, String>) -> Self {
        for (k, v) in headers {
            self.headers.insert(k, v);
        }
        self
    }

    /// Send the request, invoke hooks, validate status.
    ///
    /// On success returns the raw `reqwest::Response` whose body stream
    /// can be fed into `SseDecoder`.
    ///
    /// # Retry policy
    ///
    /// Automatically retries transient errors with exponential backoff:
    /// - max retries = `self.options.max_retries` (default: 3)
    /// - base delay = 1s, doubled each attempt, capped at `max_retry_delay_ms`
    /// - retryable: 429, 502, 503, 504, network errors, timeouts
    pub async fn send(self) -> Result<reqwest::Response, LlmError> {
        let max_retries = self.options.max_retries;
        let max_delay = std::time::Duration::from_millis(self.options.max_retry_delay_ms);
        let mut last_error: Option<LlmError> = None;

        for attempt in 0..=max_retries {
            match self.try_send_once().await {
                Ok(response) => {
                    if attempt > 0 {
                        tracing::info!(
                            provider = %self.fallback_model.provider,
                            retry_count = attempt,
                            "provider request succeeded after retries"
                        );
                    }
                    return Ok(response);
                }
                Err(e) if attempt < max_retries && e.is_retryable() => {
                    let delay = Self::retry_delay(attempt, max_delay);
                    tracing::warn!(
                        provider = %self.fallback_model.provider,
                        error = %e,
                        retry_attempt = attempt + 1,
                        max_retries = max_retries,
                        delay_ms = delay.as_millis() as u64,
                        "provider request failed, retrying"
                    );
                    last_error = Some(e);
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    tracing::error!(
                        provider = %self.fallback_model.provider,
                        error = %e,
                        retry_attempt = attempt,
                        max_retries = max_retries,
                        "provider request failed, retries exhausted or non-retryable"
                    );
                    return Err(e);
                }
            }
        }

        // This path is only reachable when max_retries = 0 and try_send_once fails,
        // or if the loop logic has a bug. Return the last captured error.
        Err(last_error
            .unwrap_or_else(|| LlmError::ProviderError("request failed after retries".to_string())))
    }

    /// Calculate exponential backoff delay for the given attempt.
    fn retry_delay(attempt: u32, max_delay: std::time::Duration) -> std::time::Duration {
        let base = std::time::Duration::from_secs(1);
        let factor = 2u32.saturating_pow(attempt);
        let delay = base.saturating_mul(factor);
        std::cmp::min(delay, max_delay)
    }

    /// Single attempt: build, send, validate.
    async fn try_send_once(&self) -> Result<reqwest::Response, LlmError> {
        // 1. on_payload hook
        let mut body = self.body.clone();
        if let Some(hook) = &self.options.on_payload {
            hook(&mut body, &self.fallback_model).await;
        }

        // 2. Build request
        let mut req = self
            .client
            .post(&self.url)
            .header("content-type", "application/json");

        for (k, v) in &self.headers {
            req = req.header(k, v);
        }

        if let Some(custom) = &self.options.headers {
            for (k, v) in custom {
                req = req.header(k, v);
            }
        }

        // 3. Send
        let response = match req.json(&body).send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Err(LlmError::Timeout(self.options.timeout));
            }
            Err(e) if e.is_connect() || e.is_request() => {
                return Err(LlmError::StreamError {
                    kind: crate::error::StreamErrorKind::Network,
                    message: format!("HTTP request failed: {e}"),
                });
            }
            Err(e) => {
                return Err(LlmError::ProviderError(format!("HTTP error: {e}")));
            }
        };

        let status = response.status().as_u16();
        let response_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        // 4. on_response hook (only on the final attempt or non-retryable responses)
        // Note: intermediate retryable responses skip this hook to avoid noise.
        if let Some(hook) = &self.options.on_response {
            hook(
                &crate::ProviderResponse {
                    status,
                    headers: response_headers.clone(),
                },
                &self.fallback_model,
            )
            .await;
        }

        // 5. Status check
        if !(200..300).contains(&status) {
            let body_text = response.text().await.map_err(|e| {
                LlmError::ProviderError(format!("failed to read response body: {e}"))
            })?;
            tracing::error!(
                status = %status,
                body = %body_text,
                provider = %self.fallback_model.provider,
                "HTTP error response from provider"
            );
            let msg = crate::http_error::sanitize_http_error_body(status, &body_text);
            return Err(classify_http_error(status, msg));
        }

        Ok(response)
    }
}

/// Map an HTTP error status code to the most appropriate [`LlmError`] variant.
fn classify_http_error(status: u16, message: String) -> LlmError {
    match status {
        429 => LlmError::RateLimited(message),
        502..=504 => LlmError::Overloaded(message),
        401 | 403 => LlmError::AuthError(message),
        400 | 422 => LlmError::InvalidRequest(message),
        _ => LlmError::ProviderError(message),
    }
}

/// Convenience: build a minimal fallback [`Model`] from provider metadata.
pub fn fallback_model(
    provider: &str,
    model_id: &str,
    api: &str,
    base_url: &str,
    context_window: u32,
    max_tokens: u32,
) -> Model {
    Model {
        id: model_id.to_string(),
        name: model_id.to_string(),
        api: api.to_string(),
        provider: provider.to_string(),
        base_url: base_url.to_string(),
        reasoning: true,
        input_modalities: vec![],
        cost: crate::TokenCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
        context_window,
        max_tokens,
        headers: None,
        compat: crate::models::ModelCompat::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_model() {
        let m = fallback_model(
            "openai",
            "gpt-4",
            "openai-completions",
            "https://api.openai.com",
            128_000,
            4096,
        );
        assert_eq!(m.id, "gpt-4");
        assert_eq!(m.provider, "openai");
        assert_eq!(m.context_window, 128_000);
    }

    #[test]
    fn test_retry_delay_exponential() {
        let max = std::time::Duration::from_secs(60);
        assert_eq!(
            RequestBuilder::retry_delay(0, max),
            std::time::Duration::from_secs(1)
        );
        assert_eq!(
            RequestBuilder::retry_delay(1, max),
            std::time::Duration::from_secs(2)
        );
        assert_eq!(
            RequestBuilder::retry_delay(2, max),
            std::time::Duration::from_secs(4)
        );
        assert_eq!(
            RequestBuilder::retry_delay(3, max),
            std::time::Duration::from_secs(8)
        );
    }

    #[test]
    fn test_retry_delay_capped() {
        let max = std::time::Duration::from_secs(5);
        assert_eq!(
            RequestBuilder::retry_delay(0, max),
            std::time::Duration::from_secs(1)
        );
        assert_eq!(
            RequestBuilder::retry_delay(3, max),
            std::time::Duration::from_secs(5)
        ); // 8s capped to 5s
    }

    #[test]
    fn test_classify_http_error() {
        assert!(matches!(
            classify_http_error(429, "too many".to_string()),
            LlmError::RateLimited(_)
        ));
        assert!(matches!(
            classify_http_error(503, "unavailable".to_string()),
            LlmError::Overloaded(_)
        ));
        assert!(matches!(
            classify_http_error(401, "unauthorized".to_string()),
            LlmError::AuthError(_)
        ));
        assert!(matches!(
            classify_http_error(500, "internal".to_string()),
            LlmError::ProviderError(_)
        ));
    }
}
