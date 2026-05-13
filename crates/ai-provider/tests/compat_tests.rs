use ai_provider::{
    CacheControlFormat, OpenAiCompat, ThinkingFormat, detect_openai_compat, merge_openai_compat,
};

#[test]
fn test_detect_openai_standard() {
    let compat = detect_openai_compat("openai", "https://api.openai.com/v1", "gpt-5.2");
    assert_eq!(compat.supports_store, Some(true));
    assert_eq!(compat.thinking_format, None);
}

#[test]
fn test_detect_deepseek_compat() {
    let compat = detect_openai_compat("deepseek", "https://api.deepseek.com/v1", "deepseek-chat");
    assert_eq!(compat.thinking_format, Some(ThinkingFormat::DeepSeek));
    assert_eq!(
        compat.requires_reasoning_content_on_assistant_messages,
        Some(true)
    );
    assert!(compat.reasoning_effort_map.is_some());
    let map = compat.reasoning_effort_map.unwrap();
    assert_eq!(map.get("xhigh"), Some(&"max".to_string()));
}

#[test]
fn test_detect_openrouter_anthropic_cache() {
    let compat = detect_openai_compat(
        "openrouter",
        "https://openrouter.ai/api/v1",
        "anthropic/claude-sonnet-4",
    );
    assert_eq!(compat.thinking_format, Some(ThinkingFormat::OpenRouter));
    assert_eq!(
        compat.cache_control_format,
        Some(CacheControlFormat::Anthropic)
    );
}

#[test]
fn test_detect_grok_no_reasoning_effort() {
    let compat = detect_openai_compat("xai", "https://api.x.ai/v1", "grok-3");
    assert_eq!(compat.supports_reasoning_effort, Some(false));
}

#[test]
fn test_merge_explicit_overrides_auto() {
    let baseline = detect_openai_compat("openai", "https://api.openai.com/v1", "gpt-5.2");
    let explicit = OpenAiCompat {
        supports_store: Some(false),
        ..Default::default()
    };
    let merged = merge_openai_compat(&baseline, &explicit);
    assert_eq!(merged.supports_store, Some(false)); // overridden
    assert_eq!(merged.supports_developer_role, Some(true)); // from baseline
}

#[test]
fn test_merge_fully_explicit() {
    let baseline = detect_openai_compat("openai", "https://api.openai.com/v1", "gpt-5.2");
    let explicit = OpenAiCompat {
        supports_store: Some(false),
        supports_developer_role: Some(false),
        supports_reasoning_effort: Some(false),
        ..Default::default()
    };
    let merged = merge_openai_compat(&baseline, &explicit);
    assert_eq!(merged.supports_store, Some(false));
    assert_eq!(merged.supports_developer_role, Some(false));
    assert_eq!(merged.supports_reasoning_effort, Some(false));
    // Fields not in explicit retain baseline
    assert!(merged.reasoning_effort_map.is_none());
}
