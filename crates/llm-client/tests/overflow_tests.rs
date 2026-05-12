use llm_client::{StopReason, is_context_overflow};

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
