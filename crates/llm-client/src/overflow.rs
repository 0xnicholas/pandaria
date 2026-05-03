use regex::Regex;
use std::sync::LazyLock;

use crate::types::StopReason;

static OVERFLOW_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"prompt is too long",                        // 1. Anthropic
        r"request_too_large",                         // 2. Anthropic 413
        r"input is too long for requested model",     // 3. Bedrock
        r"exceeds the context window",                // 4. OpenAI
        r"input token count.*exceeds the maximum",    // 5. Google
        r"maximum prompt length is \d+",              // 6. xAI
        r"reduce the length of the messages",         // 7. Groq
        r"maximum context length is \d+ tokens",      // 8. OpenRouter
        r"exceeds the limit of \d+",                  // 9. GitHub Copilot
        r"exceeds the available context size",        // 10. llama.cpp
        r"greater than the context length",           // 11. LM Studio
        r"context window exceeds limit",              // 12. MiniMax
        r"exceeded model token limit",                // 13. Kimi
        r"too large for model with \d+",              // 14. Mistral
        r"model_context_window_exceeded",             // 15. z.ai
        r"prompt too long.*context length",           // 16. Ollama
        r"4(00|13)\s*(?:status code)?\s*\(no body\)", // 17. Cerebras
        r"context[_ ]length[_ ]exceeded",             // 18. Generic
        r"too many tokens|token limit exceeded",      // 19. Generic fallback
    ]
    .iter()
    .map(|p| Regex::new(p).expect("overflow pattern must compile"))
    .collect()
});

static NON_OVERFLOW_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"^(Throttling error|Service unavailable):", // Bedrock throttling
        r"rate limit",                               // Generic rate limiting
        r"too many requests",                        // HTTP 429
    ]
    .iter()
    .map(|p| Regex::new(p).expect("exclusion pattern must compile"))
    .collect()
});

/// Detect whether an LLM error indicates context window overflow.
///
/// Two detection paths:
///   1. Error-based: stop_reason == Error and error_message matches overflow regex,
///      after excluding NON_OVERFLOW patterns (rate limit / throttling)
///   2. Silent overflow: stop_reason == Stop but input_tokens + cache_read_tokens > context_window
///      (handles z.ai / Ollama silent truncation)
pub fn is_context_overflow(
    error_message: Option<&str>,
    stop_reason: &StopReason,
    context_window: Option<u32>,
    input_tokens: u64,
    cache_read_tokens: u64,
) -> bool {
    // Case 1: Error-based detection
    if *stop_reason == StopReason::Error
        && let Some(msg) = error_message
    {
        let is_non_overflow = NON_OVERFLOW_PATTERNS.iter().any(|re| re.is_match(msg));
        if !is_non_overflow && OVERFLOW_PATTERNS.iter().any(|re| re.is_match(msg)) {
            return true;
        }
    }

    // Case 2: Silent overflow (z.ai / Ollama style)
    if *stop_reason == StopReason::Stop
        && let Some(cw) = context_window
    {
        let input = input_tokens + cache_read_tokens;
        if input > cw as u64 {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anthropic_prompt_too_long() {
        assert!(is_context_overflow(
            Some("prompt is too long: 213462 tokens > 200000"),
            &StopReason::Error,
            None,
            0,
            0,
        ));
    }

    #[test]
    fn test_openai_exceeds_context_window() {
        assert!(is_context_overflow(
            Some("Your input exceeds the context window"),
            &StopReason::Error,
            None,
            0,
            0,
        ));
    }

    #[test]
    fn test_google_input_exceeds_maximum() {
        assert!(is_context_overflow(
            Some("The input token count (1196265) exceeds the maximum"),
            &StopReason::Error,
            None,
            0,
            0,
        ));
    }

    #[test]
    fn test_generic_context_length_exceeded() {
        assert!(is_context_overflow(
            Some("context_length_exceeded: the request exceeds the available context size"),
            &StopReason::Error,
            None,
            0,
            0,
        ));
    }

    #[test]
    fn test_non_overflow_throttling_excluded() {
        assert!(!is_context_overflow(
            Some("ThrottlingException: Too many tokens, please wait..."),
            &StopReason::Error,
            None,
            0,
            0,
        ));
    }

    #[test]
    fn test_non_overflow_rate_limit_excluded() {
        assert!(!is_context_overflow(
            Some("rate limit exceeded, please retry"),
            &StopReason::Error,
            None,
            0,
            0,
        ));
    }

    #[test]
    fn test_silent_overflow_detection() {
        assert!(is_context_overflow(
            None,
            &StopReason::Stop,
            Some(1000),
            1200,
            0,
        ));
    }

    #[test]
    fn test_no_overflow_on_normal_stop() {
        assert!(!is_context_overflow(
            None,
            &StopReason::Stop,
            Some(2000),
            1200,
            0,
        ));
    }

    #[test]
    fn test_no_overflow_on_tool_use() {
        assert!(!is_context_overflow(
            None,
            &StopReason::ToolUse,
            Some(1000),
            2000,
            0,
        ));
    }
}
