use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ━━━ OpenAI Chat Completions Compatibility ━━━

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MaxTokensField {
    #[serde(rename = "max_completion_tokens")]
    MaxCompletionTokens,
    #[serde(rename = "max_tokens")]
    MaxTokens,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThinkingFormat {
    OpenAI,
    OpenRouter,
    DeepSeek,
    Zai,
    Qwen,
    QwenChatTemplate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CacheControlFormat {
    Anthropic,
}

/// OpenRouter provider routing preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenRouterRouting {
    pub allow_fallbacks: Option<bool>,
    pub order: Option<Vec<String>>,
    pub only: Option<Vec<String>>,
    pub sort: Option<String>,
    pub max_price: Option<f64>,
    pub quantizations: Option<Vec<String>>,
}

/// Vercel AI Gateway routing preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VercelGatewayRouting {
    pub only: Option<Vec<String>>,
    pub order: Option<Vec<String>>,
}

/// OpenAI Chat Completions API compatibility overrides.
/// All fields are Option: None uses auto-detected defaults, Some(v) overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenAiCompat {
    pub supports_store: Option<bool>,
    pub supports_developer_role: Option<bool>,
    pub supports_reasoning_effort: Option<bool>,
    /// Keys: "minimal", "low", "medium", "high", "xhigh"
    /// Example: DeepSeek maps all to "high", "xhigh"→"max"
    pub reasoning_effort_map: Option<HashMap<String, String>>,
    pub supports_usage_in_streaming: Option<bool>,
    pub max_tokens_field: Option<MaxTokensField>,
    pub requires_tool_result_name: Option<bool>,
    pub requires_assistant_after_tool_result: Option<bool>,
    pub requires_thinking_as_text: Option<bool>,
    pub requires_reasoning_content_on_assistant_messages: Option<bool>,
    pub thinking_format: Option<ThinkingFormat>,
    pub supports_strict_mode: Option<bool>,
    pub cache_control_format: Option<CacheControlFormat>,
    pub send_session_affinity_headers: Option<bool>,
    pub supports_long_cache_retention: Option<bool>,
    pub zai_tool_stream: Option<bool>,
    pub open_router_routing: Option<OpenRouterRouting>,
    pub vercel_gateway_routing: Option<VercelGatewayRouting>,
}

// ━━━ Anthropic Messages Compatibility ━━━

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AnthropicCompat {
    pub supports_eager_tool_input_streaming: Option<bool>,
    pub supports_long_cache_retention: Option<bool>,
}

// ━━━ Auto-detection ━━━

pub fn detect_openai_compat(provider: &str, base_url: &str, model_id: &str) -> OpenAiCompat {
    let is_non_standard = provider == "cerebras"
        || provider == "xai"
        || provider == "deepseek"
        || provider == "zai"
        || provider == "opencode"
        || provider == "cloudflare-workers-ai"
        || provider == "doubao"
        || base_url.contains("cerebras.ai")
        || base_url.contains("api.x.ai")
        || base_url.contains("deepseek.com")
        || base_url.contains("api.z.ai")
        || base_url.contains("opencode.ai")
        || base_url.contains("api.cloudflare.com")
        || base_url.contains("volces.com");

    let is_deepseek = provider == "deepseek" || base_url.contains("deepseek.com");
    let is_grok = provider == "xai" || base_url.contains("api.x.ai");
    let is_zai = provider == "zai" || base_url.contains("api.z.ai");
    let is_openrouter = provider == "openrouter" || base_url.contains("openrouter.ai");

    let cache_control_format = if is_openrouter && model_id.starts_with("anthropic/") {
        Some(CacheControlFormat::Anthropic)
    } else {
        None
    };

    let thinking_format = if is_deepseek {
        Some(ThinkingFormat::DeepSeek)
    } else if is_zai {
        Some(ThinkingFormat::Zai)
    } else if is_openrouter {
        Some(ThinkingFormat::OpenRouter)
    } else {
        None
    };

    let reasoning_effort_map = if is_deepseek {
        let mut m = HashMap::new();
        m.insert("minimal".to_string(), "high".to_string());
        m.insert("low".to_string(), "high".to_string());
        m.insert("medium".to_string(), "high".to_string());
        m.insert("high".to_string(), "high".to_string());
        m.insert("xhigh".to_string(), "max".to_string());
        Some(m)
    } else {
        None
    };

    OpenAiCompat {
        supports_store: Some(!is_non_standard),
        supports_developer_role: Some(!is_non_standard),
        supports_reasoning_effort: Some(!is_grok && !is_zai),
        reasoning_effort_map,
        supports_usage_in_streaming: None,
        max_tokens_field: None,
        requires_tool_result_name: None,
        requires_assistant_after_tool_result: None,
        requires_thinking_as_text: None,
        requires_reasoning_content_on_assistant_messages: if is_deepseek {
            Some(true)
        } else {
            None
        },
        thinking_format,
        supports_strict_mode: None,
        cache_control_format,
        send_session_affinity_headers: None,
        supports_long_cache_retention: None,
        zai_tool_stream: None,
        open_router_routing: None,
        vercel_gateway_routing: None,
    }
}

pub fn detect_anthropic_compat(_provider: &str, _base_url: &str) -> AnthropicCompat {
    AnthropicCompat::default()
}

// ━━━ Merge ━━━

fn opt_or<T: Clone>(explicit: &Option<T>, baseline: &Option<T>) -> Option<T> {
    if explicit.is_some() {
        explicit.clone()
    } else {
        baseline.clone()
    }
}

pub fn merge_openai_compat(baseline: &OpenAiCompat, explicit: &OpenAiCompat) -> OpenAiCompat {
    OpenAiCompat {
        supports_store: opt_or(&explicit.supports_store, &baseline.supports_store),
        supports_developer_role: opt_or(
            &explicit.supports_developer_role,
            &baseline.supports_developer_role,
        ),
        supports_reasoning_effort: opt_or(
            &explicit.supports_reasoning_effort,
            &baseline.supports_reasoning_effort,
        ),
        reasoning_effort_map: opt_or(
            &explicit.reasoning_effort_map,
            &baseline.reasoning_effort_map,
        ),
        supports_usage_in_streaming: opt_or(
            &explicit.supports_usage_in_streaming,
            &baseline.supports_usage_in_streaming,
        ),
        max_tokens_field: opt_or(&explicit.max_tokens_field, &baseline.max_tokens_field),
        requires_tool_result_name: opt_or(
            &explicit.requires_tool_result_name,
            &baseline.requires_tool_result_name,
        ),
        requires_assistant_after_tool_result: opt_or(
            &explicit.requires_assistant_after_tool_result,
            &baseline.requires_assistant_after_tool_result,
        ),
        requires_thinking_as_text: opt_or(
            &explicit.requires_thinking_as_text,
            &baseline.requires_thinking_as_text,
        ),
        requires_reasoning_content_on_assistant_messages: opt_or(
            &explicit.requires_reasoning_content_on_assistant_messages,
            &baseline.requires_reasoning_content_on_assistant_messages,
        ),
        thinking_format: opt_or(&explicit.thinking_format, &baseline.thinking_format),
        supports_strict_mode: opt_or(
            &explicit.supports_strict_mode,
            &baseline.supports_strict_mode,
        ),
        cache_control_format: opt_or(
            &explicit.cache_control_format,
            &baseline.cache_control_format,
        ),
        send_session_affinity_headers: opt_or(
            &explicit.send_session_affinity_headers,
            &baseline.send_session_affinity_headers,
        ),
        supports_long_cache_retention: opt_or(
            &explicit.supports_long_cache_retention,
            &baseline.supports_long_cache_retention,
        ),
        zai_tool_stream: opt_or(&explicit.zai_tool_stream, &baseline.zai_tool_stream),
        open_router_routing: opt_or(&explicit.open_router_routing, &baseline.open_router_routing),
        vercel_gateway_routing: opt_or(
            &explicit.vercel_gateway_routing,
            &baseline.vercel_gateway_routing,
        ),
    }
}

pub fn merge_anthropic_compat(
    baseline: &AnthropicCompat,
    explicit: &AnthropicCompat,
) -> AnthropicCompat {
    AnthropicCompat {
        supports_eager_tool_input_streaming: opt_or(
            &explicit.supports_eager_tool_input_streaming,
            &baseline.supports_eager_tool_input_streaming,
        ),
        supports_long_cache_retention: opt_or(
            &explicit.supports_long_cache_retention,
            &baseline.supports_long_cache_retention,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_openai_standard() {
        let compat = detect_openai_compat("openai", "https://api.openai.com/v1", "gpt-5.2");
        assert_eq!(compat.supports_store, Some(true));
        assert_eq!(compat.thinking_format, None);
    }

    #[test]
    fn test_detect_deepseek_compat() {
        let compat =
            detect_openai_compat("deepseek", "https://api.deepseek.com", "deepseek-chat");
        assert_eq!(compat.thinking_format, Some(ThinkingFormat::DeepSeek));
        assert_eq!(
            compat.requires_reasoning_content_on_assistant_messages,
            Some(true)
        );
        assert!(compat.reasoning_effort_map.is_some());
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
    fn test_detect_doubao_compat() {
        let compat = detect_openai_compat("doubao", "https://ark.cn-beijing.volces.com/api/v3/chat/completions", "doubao-pro-32k");
        assert_eq!(compat.supports_store, Some(false));
        assert_eq!(compat.supports_developer_role, Some(false));
        assert_eq!(compat.thinking_format, None);
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
        // Fields not in explicit retain baseline
        assert!(merged.reasoning_effort_map.is_none());
    }

    #[test]
    fn test_detect_anthropic_always_default() {
        // detect_anthropic_compat currently ignores its arguments and
        // always returns the default AnthropicCompat. If this function
        // gains detection logic in the future, this test should be updated.
        let result = detect_anthropic_compat("anthropic", "https://api.anthropic.com");
        assert_eq!(result, AnthropicCompat::default());

        let result2 = detect_anthropic_compat("custom", "https://custom.proxy.io");
        assert_eq!(result2, AnthropicCompat::default());
    }
}
