use std::collections::HashMap;

use secrecy::SecretString;

use crate::compat::{CacheControlFormat, OpenAiCompat};
use crate::error::LlmError;
use crate::models::ModelCompat;

/// 解析结果，包含目标 provider 名、实际 model_id、base_url、compat 覆盖等。
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    /// 底层 provider 名称（如 "openai", "anthropic", "openrouter"）
    pub provider_name: String,
    /// 传给底层 provider stream() 的实际 model_id
    pub model_id: String,
    /// 覆盖的 base_url（如 Ollama 的 localhost）
    pub base_url: Option<String>,
    /// 覆盖的 API key
    pub api_key: Option<SecretString>,
    /// 额外 headers
    pub headers: Option<HashMap<String, String>>,
    /// Compat 覆盖
    pub compat: Option<ModelCompat>,
    /// 底层 API 协议标识（如 "openai-completions"、"anthropic-messages"）
    pub api_type: String,
}

/// 创建底层 provider 实例的工厂。
#[derive(Debug, Clone)]
pub enum ProviderFactory {
    OpenAi,
    Anthropic,
    Google,
    DeepSeek,
    Mistral,
    Doubao,
    /// 用于 OpenRouter / Ollama / 自定义代理等 OpenAI-compatible 端点
    OpenAiCompatible {
        provider_name: String,
        env_key: &'static str,
    },
}

/// 单条 provider 解析规则。
#[derive(Debug, Clone)]
pub struct ProviderRule {
    pub factory: ProviderFactory,
    pub default_base_url: String,
    pub env_key: &'static str,
    pub api_type: &'static str,
    pub compat_hints: Option<ModelCompat>,
    pub fallback_context_window: u32,
    pub fallback_max_tokens: u32,
}

/// 将 Model Spec 解析为 ResolvedModel 的纯函数组件。
#[derive(Debug, Clone)]
pub struct ProviderResolver {
    /// 内置规则表：provider_name → ProviderRule
    rules: HashMap<String, ProviderRule>,
    /// 用户自定义覆盖
    custom: HashMap<String, ProviderRule>,
}

impl Default for ProviderResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderResolver {
    /// 创建默认解析器，内置所有已知 provider 规则。
    pub fn new() -> Self {
        Self {
            rules: Self::build_builtin_rules(),
            custom: HashMap::new(),
        }
    }

    /// 注册自定义 provider 规则（覆盖内置规则）。
    pub fn register(&mut self, name: String, rule: ProviderRule) {
        self.custom.insert(name, rule);
    }

    /// 解析 model spec（如 "openai/gpt-5.2"）为 ResolvedModel。
    pub fn resolve(&self, model_spec: &str) -> Result<ResolvedModel, LlmError> {
        let mut segments = model_spec.splitn(2, '/');
        let provider = segments
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| LlmError::InvalidRequest(format!("invalid model spec: {model_spec}")))?;
        let model_id = segments
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| LlmError::InvalidRequest(format!("invalid model spec: {model_spec}")))?;

        match provider {
            "openrouter" => self.resolve_openrouter(model_id),
            "ollama" => self.resolve_ollama(model_id),
            _ => self.resolve_standard(provider, model_id),
        }
    }

    /// 获取指定 provider 的规则。
    pub fn get_rule(&self, provider_name: &str) -> Result<&ProviderRule, LlmError> {
        self.custom
            .get(provider_name)
            .or_else(|| self.rules.get(provider_name))
            .ok_or_else(|| LlmError::UnknownProvider(provider_name.to_string()))
    }

    /// 获取指定 provider 的默认 base_url。
    pub fn default_base_url(&self, provider_name: &str) -> String {
        self.get_rule(provider_name)
            .map(|r| r.default_base_url.clone())
            .unwrap_or_default()
    }

    fn resolve_standard(&self, provider: &str, model_id: &str) -> Result<ResolvedModel, LlmError> {
        let rule = self.get_rule(provider)?;
        Ok(ResolvedModel {
            provider_name: provider.to_string(),
            model_id: model_id.to_string(),
            base_url: None,
            api_key: None,
            headers: None,
            compat: rule.compat_hints.clone(),
            api_type: rule.api_type.to_string(),
        })
    }

    fn resolve_openrouter(&self, model_id: &str) -> Result<ResolvedModel, LlmError> {
        let rule = self.get_rule("openrouter")?;

        // 提取 underlying provider hint（第二个 segment）
        let underlying = model_id.split('/').next().unwrap_or("");

        // 若 underlying 为 anthropic，api_type 为 anthropic-messages，否则为 openai-completions
        let api_type = if underlying == "anthropic" {
            "anthropic-messages"
        } else {
            "openai-completions"
        };

        // 若 underlying 为 anthropic，注入 cache_control_format compat
        let compat = if underlying == "anthropic" {
            Some(ModelCompat::OpenAI(OpenAiCompat {
                cache_control_format: Some(CacheControlFormat::Anthropic),
                ..Default::default()
            }))
        } else {
            rule.compat_hints.clone()
        };

        Ok(ResolvedModel {
            provider_name: "openrouter".to_string(),
            model_id: model_id.to_string(),
            base_url: Some(rule.default_base_url.clone()),
            api_key: None,
            headers: None,
            compat,
            api_type: api_type.to_string(),
        })
    }

    fn resolve_ollama(&self, model_id: &str) -> Result<ResolvedModel, LlmError> {
        let rule = self.get_rule("ollama")?;

        // base_url 优先级：OLLAMA_HOST env → localhost 默认
        let host = std::env::var("OLLAMA_HOST").ok();
        let base_url = build_ollama_base_url(host.as_deref());

        Ok(ResolvedModel {
            provider_name: "ollama".to_string(),
            model_id: model_id.to_string(),
            base_url: Some(base_url),
            api_key: None,
            headers: None,
            compat: rule.compat_hints.clone(),
            api_type: rule.api_type.to_string(),
        })
    }

    fn build_builtin_rules() -> HashMap<String, ProviderRule> {
        let mut rules = HashMap::new();

        rules.insert(
            "openai".to_string(),
            ProviderRule {
                factory: ProviderFactory::OpenAi,
                default_base_url: "https://api.openai.com/v1/chat/completions".to_string(),
                env_key: "OPENAI_API_KEY",
                api_type: "openai-completions",
                compat_hints: Some(ModelCompat::OpenAI(OpenAiCompat::default())),
                fallback_context_window: 128_000,
                fallback_max_tokens: 128_000,
            },
        );

        rules.insert(
            "anthropic".to_string(),
            ProviderRule {
                factory: ProviderFactory::Anthropic,
                default_base_url: "https://api.anthropic.com/v1/messages".to_string(),
                env_key: "ANTHROPIC_API_KEY",
                api_type: "anthropic-messages",
                compat_hints: Some(ModelCompat::Anthropic(
                    crate::compat::AnthropicCompat::default(),
                )),
                fallback_context_window: 200_000,
                fallback_max_tokens: 8192,
            },
        );

        rules.insert(
            "google".to_string(),
            ProviderRule {
                factory: ProviderFactory::Google,
                default_base_url: "https://generativelanguage.googleapis.com/v1beta/models"
                    .to_string(),
                env_key: "GOOGLE_API_KEY",
                api_type: "google-generative",
                compat_hints: None,
                fallback_context_window: 1_048_576,
                fallback_max_tokens: 8192,
            },
        );

        rules.insert(
            "deepseek".to_string(),
            ProviderRule {
                factory: ProviderFactory::DeepSeek,
                default_base_url: "https://api.deepseek.com/chat/completions".to_string(),
                env_key: "DEEPSEEK_API_KEY",
                api_type: "openai-completions",
                compat_hints: Some(ModelCompat::OpenAI(OpenAiCompat {
                    thinking_format: Some(crate::compat::ThinkingFormat::DeepSeek),
                    requires_reasoning_content_on_assistant_messages: Some(true),
                    reasoning_effort_map: Some({
                        let mut m = HashMap::new();
                        m.insert("minimal".to_string(), "high".to_string());
                        m.insert("low".to_string(), "high".to_string());
                        m.insert("medium".to_string(), "high".to_string());
                        m.insert("high".to_string(), "high".to_string());
                        m.insert("xhigh".to_string(), "max".to_string());
                        m
                    }),
                    ..Default::default()
                })),
                fallback_context_window: 128_000,
                fallback_max_tokens: 128_000,
            },
        );

        rules.insert(
            "mistral".to_string(),
            ProviderRule {
                factory: ProviderFactory::Mistral,
                default_base_url: "https://api.mistral.ai/v1/chat/completions".to_string(),
                env_key: "MISTRAL_API_KEY",
                api_type: "openai-completions",
                compat_hints: Some(ModelCompat::OpenAI(OpenAiCompat::default())),
                fallback_context_window: 128_000,
                fallback_max_tokens: 128_000,
            },
        );

        rules.insert(
            "doubao".to_string(),
            ProviderRule {
                factory: ProviderFactory::Doubao,
                default_base_url: "https://ark.cn-beijing.volces.com/api/v3/chat/completions"
                    .to_string(),
                env_key: "DOUBAO_API_KEY",
                api_type: "openai-completions",
                compat_hints: Some(ModelCompat::OpenAI(OpenAiCompat::default())),
                fallback_context_window: 262_144,
                fallback_max_tokens: 4096,
            },
        );

        rules.insert(
            "openrouter".to_string(),
            ProviderRule {
                factory: ProviderFactory::OpenAiCompatible {
                    provider_name: "openrouter".to_string(),
                    env_key: "OPENROUTER_API_KEY",
                },
                default_base_url: "https://openrouter.ai/api/v1/chat/completions".to_string(),
                env_key: "OPENROUTER_API_KEY",
                api_type: "openai-completions",
                compat_hints: None,
                fallback_context_window: 128_000,
                fallback_max_tokens: 128_000,
            },
        );

        rules.insert(
            "ollama".to_string(),
            ProviderRule {
                factory: ProviderFactory::OpenAiCompatible {
                    provider_name: "ollama".to_string(),
                    env_key: "OLLAMA_API_KEY",
                },
                default_base_url: "http://localhost:11434/v1/chat/completions".to_string(),
                env_key: "OLLAMA_API_KEY",
                api_type: "openai-completions",
                compat_hints: Some(ModelCompat::OpenAI(OpenAiCompat::default())),
                fallback_context_window: 128_000,
                fallback_max_tokens: 128_000,
            },
        );

        rules.insert(
            "mimo".to_string(),
            ProviderRule {
                factory: ProviderFactory::OpenAiCompatible {
                    provider_name: "mimo".to_string(),
                    env_key: "MIMO_API_KEY",
                },
                default_base_url: "https://api.xiaomimimo.com/v1/chat/completions"
                    .to_string(),
                env_key: "MIMO_API_KEY",
                api_type: "openai-completions",
                compat_hints: None,
                fallback_context_window: 1_048_576,
                fallback_max_tokens: 128_000,
            },
        );

        rules
    }
}

/// Build Ollama base_url from optional host string.
fn build_ollama_base_url(host: Option<&str>) -> String {
    match host {
        Some(h) => {
            if h.ends_with("/v1/chat/completions") {
                h.to_string()
            } else if h.ends_with("/v1") {
                format!("{}/chat/completions", h)
            } else if h.ends_with('/') {
                format!("{}v1/chat/completions", h)
            } else {
                format!("{}/v1/chat/completions", h)
            }
        }
        None => "http://localhost:11434/v1/chat/completions".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_standard_openai() {
        let resolver = ProviderResolver::new();
        let resolved = resolver.resolve("openai/gpt-5.2").unwrap();
        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model_id, "gpt-5.2");
        assert_eq!(resolved.api_type, "openai-completions");
        assert!(resolved.base_url.is_none());
    }

    #[test]
    fn test_resolve_standard_anthropic() {
        let resolver = ProviderResolver::new();
        let resolved = resolver
            .resolve("anthropic/claude-sonnet-4-20250514")
            .unwrap();
        assert_eq!(resolved.provider_name, "anthropic");
        assert_eq!(resolved.model_id, "claude-sonnet-4-20250514");
        assert_eq!(resolved.api_type, "anthropic-messages");
    }

    #[test]
    fn test_resolve_standard_google() {
        let resolver = ProviderResolver::new();
        let resolved = resolver.resolve("google/gemini-2.5-pro").unwrap();
        assert_eq!(resolved.provider_name, "google");
        assert_eq!(resolved.model_id, "gemini-2.5-pro");
        assert_eq!(resolved.api_type, "google-generative");
    }

    #[test]
    fn test_resolve_standard_deepseek() {
        let resolver = ProviderResolver::new();
        let resolved = resolver.resolve("deepseek/deepseek-chat").unwrap();
        assert_eq!(resolved.provider_name, "deepseek");
        assert_eq!(resolved.model_id, "deepseek-chat");
        assert_eq!(resolved.api_type, "openai-completions");
        assert!(matches!(
            resolved.compat,
            Some(ModelCompat::OpenAI(OpenAiCompat {
                thinking_format: Some(crate::compat::ThinkingFormat::DeepSeek),
                ..
            }))
        ));
    }

    #[test]
    fn test_resolve_standard_mistral() {
        let resolver = ProviderResolver::new();
        let resolved = resolver.resolve("mistral/mistral-large").unwrap();
        assert_eq!(resolved.provider_name, "mistral");
        assert_eq!(resolved.model_id, "mistral-large");
        assert_eq!(resolved.api_type, "openai-completions");
    }

    #[test]
    fn test_resolve_standard_doubao() {
        let resolver = ProviderResolver::new();
        let resolved = resolver.resolve("doubao/doubao-pro-32k").unwrap();
        assert_eq!(resolved.provider_name, "doubao");
        assert_eq!(resolved.model_id, "doubao-pro-32k");
        assert_eq!(resolved.api_type, "openai-completions");
        assert!(matches!(
            resolved.compat,
            Some(ModelCompat::OpenAI(OpenAiCompat { .. }))
        ));
    }

    #[test]
    fn test_resolve_openrouter_nested() {
        let resolver = ProviderResolver::new();
        let resolved = resolver
            .resolve("openrouter/anthropic/claude-sonnet-4")
            .unwrap();
        assert_eq!(resolved.provider_name, "openrouter");
        assert_eq!(resolved.model_id, "anthropic/claude-sonnet-4");
        assert_eq!(resolved.api_type, "anthropic-messages");
        assert_eq!(
            resolved.base_url,
            Some("https://openrouter.ai/api/v1/chat/completions".to_string())
        );
        assert!(matches!(
            resolved.compat,
            Some(ModelCompat::OpenAI(OpenAiCompat {
                cache_control_format: Some(CacheControlFormat::Anthropic),
                ..
            }))
        ));
    }

    #[test]
    fn test_resolve_openrouter_non_anthropic() {
        let resolver = ProviderResolver::new();
        let resolved = resolver.resolve("openrouter/openai/gpt-5.2").unwrap();
        assert_eq!(resolved.provider_name, "openrouter");
        assert_eq!(resolved.model_id, "openai/gpt-5.2");
        assert_eq!(resolved.api_type, "openai-completions");
        assert!(resolved.compat.is_none());
    }

    #[test]
    fn test_resolve_ollama_default() {
        let resolver = ProviderResolver::new();
        let resolved = resolver.resolve("ollama/llama3.1").unwrap();
        assert_eq!(resolved.provider_name, "ollama");
        assert_eq!(resolved.model_id, "llama3.1");
        assert_eq!(
            resolved.base_url,
            Some("http://localhost:11434/v1/chat/completions".to_string())
        );
        assert_eq!(resolved.api_type, "openai-completions");
    }

    #[test]
    fn test_build_ollama_base_url_default() {
        assert_eq!(
            build_ollama_base_url(None),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn test_build_ollama_base_url_custom_host() {
        assert_eq!(
            build_ollama_base_url(Some("http://192.168.1.100:11434")),
            "http://192.168.1.100:11434/v1/chat/completions"
        );
    }

    #[test]
    fn test_build_ollama_base_url_trailing_slash() {
        assert_eq!(
            build_ollama_base_url(Some("http://localhost:11434/")),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn test_build_ollama_base_url_v1_suffix() {
        assert_eq!(
            build_ollama_base_url(Some("http://localhost:11434/v1")),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn test_build_ollama_base_url_full_path() {
        assert_eq!(
            build_ollama_base_url(Some("http://localhost:11434/v1/chat/completions")),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn test_resolve_unknown_provider() {
        let resolver = ProviderResolver::new();
        let result = resolver.resolve("unknown/model");
        assert!(matches!(result, Err(LlmError::UnknownProvider(_))));
    }

    #[test]
    fn test_resolve_invalid_spec_no_slash() {
        let resolver = ProviderResolver::new();
        let result = resolver.resolve("openai");
        assert!(matches!(result, Err(LlmError::InvalidRequest(_))));
    }

    #[test]
    fn test_resolve_invalid_spec_empty_model() {
        let resolver = ProviderResolver::new();
        let result = resolver.resolve("openai/");
        assert!(matches!(result, Err(LlmError::InvalidRequest(_))));
    }

    #[test]
    fn test_get_rule_builtin() {
        let resolver = ProviderResolver::new();
        let rule = resolver.get_rule("openai").unwrap();
        assert!(matches!(rule.factory, ProviderFactory::OpenAi));
        assert_eq!(rule.env_key, "OPENAI_API_KEY");
    }

    #[test]
    fn test_get_rule_unknown() {
        let resolver = ProviderResolver::new();
        let result = resolver.get_rule("nonexistent");
        assert!(matches!(result, Err(LlmError::UnknownProvider(_))));
    }

    #[test]
    fn test_custom_override() {
        let mut resolver = ProviderResolver::new();
        let custom_rule = ProviderRule {
            factory: ProviderFactory::OpenAiCompatible {
                provider_name: "custom".to_string(),
                env_key: "CUSTOM_API_KEY",
            },
            default_base_url: "https://custom.example.com/v1".to_string(),
            env_key: "CUSTOM_API_KEY",
            api_type: "openai-completions",
            compat_hints: None,
            fallback_context_window: 128_000,
            fallback_max_tokens: 128_000,
        };
        resolver.register("custom".to_string(), custom_rule);
        let resolved = resolver.resolve("custom/my-model").unwrap();
        assert_eq!(resolved.provider_name, "custom");
        assert_eq!(resolved.api_type, "openai-completions");
    }

    #[test]
    fn test_default_base_url() {
        let resolver = ProviderResolver::new();
        assert_eq!(
            resolver.default_base_url("openai"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(resolver.default_base_url("unknown"), "");
    }

    #[test]
    fn test_resolve_mimo() {
        let resolver = ProviderResolver::new();
        let resolved = resolver.resolve("mimo/mimo-v2.5-pro").unwrap();
        assert_eq!(resolved.provider_name, "mimo");
        assert_eq!(resolved.model_id, "mimo-v2.5-pro");
        assert_eq!(resolved.api_type, "openai-completions");
    }
}
