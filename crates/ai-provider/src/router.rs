use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::models::{Model, ModelCompat, Modality, TokenCost, get_model};
use crate::provider::{LlmProvider, StreamOptions};
use crate::providers::shared::ProviderConfig;
use crate::resolver::{ProviderFactory, ProviderResolver, ResolvedModel};
use crate::streaming::AssistantMessageEventStream;
use crate::types::LlmContext;

/// 统一路由入口，将 `provider/model` 格式的标识符解析并路由到正确的底层 provider。
///
/// 对 `agent-core` 完全透明——`SessionActor` 仍只持有一个 `Arc<dyn LlmProvider>`。
pub struct RouterProvider {
    resolver: ProviderResolver,
    default_config: ProviderConfig,
    cache: DashMap<(String, String), Arc<dyn LlmProvider>>,
}

impl RouterProvider {
    /// 创建新的 RouterProvider，内置所有已知 provider 规则。
    pub fn new() -> Self {
        Self {
            resolver: ProviderResolver::new(),
            default_config: ProviderConfig::new(
                None,
                "http://router",
                "router",
                "ROUTER_API_KEY",
            ),
            cache: DashMap::new(),
        }
    }

    /// 获取或创建底层 provider 实例（按 `(provider_name, base_url)` 缓存）。
    fn get_or_create_provider(
        &self,
        provider_name: &str,
        base_url: &str,
    ) -> Result<Arc<dyn LlmProvider>, LlmError> {
        let key = (provider_name.to_string(), base_url.to_string());

        if let Some(entry) = self.cache.get(&key) {
            return Ok(entry.clone());
        }

        let rule = self.resolver.get_rule(provider_name)?;
        let instance: Arc<dyn LlmProvider> = match &rule.factory {
            ProviderFactory::OpenAi => Arc::new(crate::providers::openai::OpenAiProvider::with_base_url(None, base_url)),
            ProviderFactory::Anthropic => Arc::new(crate::providers::anthropic::AnthropicProvider::with_base_url(None, base_url)),
            ProviderFactory::Google => Arc::new(crate::providers::google::GoogleProvider::with_base_url(None, base_url)),
            ProviderFactory::DeepSeek => Arc::new(crate::providers::deepseek::DeepSeekProvider::with_base_url(None, base_url)),
            ProviderFactory::Mistral => Arc::new(crate::providers::mistral::MistralProvider::with_base_url(None, base_url)),
            ProviderFactory::OpenAiCompatible { provider_name: name, env_key } => {
                Arc::new(crate::providers::openai_compatible::OpenAiCompatibleProvider::new(
                    None, base_url, name, env_key,
                ))
            }
        };

        self.cache.insert(key, instance.clone());
        Ok(instance)
    }

    /// 当模型不在静态注册表中时，用 ProviderRule 的默认值构建 fallback Model。
    fn build_fallback_model(&self, resolved: &ResolvedModel) -> Option<Model> {
        let rule = self.resolver.get_rule(&resolved.provider_name).ok()?;
        let base_url = resolved.base_url.clone()
            .unwrap_or_else(|| self.resolver.default_base_url(&resolved.provider_name));
        Some(Model {
            id: resolved.model_id.clone(),
            name: resolved.model_id.clone(),
            api: resolved.api_type.clone(),
            provider: resolved.provider_name.clone(),
            base_url,
            reasoning: false,
            input_modalities: vec![Modality::Text],
            cost: TokenCost::default(),
            context_window: rule.fallback_context_window,
            max_tokens: rule.fallback_max_tokens,
            headers: None,
            compat: rule.compat_hints.clone().unwrap_or(ModelCompat::None),
        })
    }
}

impl Default for RouterProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmProvider for RouterProvider {
    fn provider_name(&self) -> &str {
        "router"
    }

    fn config(&self) -> &ProviderConfig {
        &self.default_config
    }

    fn models(&self) -> Vec<String> {
        let mut result = Vec::new();
        for (provider, model_ids) in crate::models_data::PROVIDER_MODELS.iter() {
            for id in model_ids.iter() {
                result.push(format!("{}/{}", provider, id));
            }
        }
        result
    }

    fn model_metadata(&self, model: &str) -> Option<Model> {
        let resolved = self.resolver.resolve(model).ok()?;

        // OpenRouter 特殊处理：用 underlying provider 查注册表
        if resolved.provider_name == "openrouter" {
            let mut segments = resolved.model_id.splitn(2, '/');
            let underlying = segments.next()?;
            let actual_model = segments.next()?;
            return get_model(underlying, actual_model)
                .or_else(|| self.build_fallback_model(&resolved));
        }

        get_model(&resolved.provider_name, &resolved.model_id)
            .or_else(|| self.build_fallback_model(&resolved))
    }

    async fn stream(
        &self,
        model: &str,
        context: LlmContext,
        options: StreamOptions,
        signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError> {
        let resolved = self.resolver.resolve(model)?;

        // 确定实际 base_url：resolved 覆盖 > 规则表默认值
        let base_url = resolved.base_url.clone()
            .unwrap_or_else(|| {
                self.resolver.default_base_url(&resolved.provider_name)
            });

        let provider = self.get_or_create_provider(
            &resolved.provider_name,
            &base_url,
        )?;

        // 合并 resolved 中的 overrides 到 options
        let mut opts = options;
        if let Some(key) = resolved.api_key {
            opts.api_key = Some(key);
        }
        if let Some(h) = resolved.headers {
            opts.headers = Some(h);
        }

        provider.stream(&resolved.model_id, context, opts, signal).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_name() {
        let router = RouterProvider::new();
        assert_eq!(router.provider_name(), "router");
    }

    #[test]
    fn test_models_aggregated() {
        let router = RouterProvider::new();
        let models = router.models();
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m == "openai/gpt-5.2"));
        assert!(models.iter().any(|m| m == "anthropic/claude-sonnet-4-20250514"));
    }

    #[test]
    fn test_model_metadata_openai() {
        let router = RouterProvider::new();
        let model = router.model_metadata("openai/gpt-5.2").unwrap();
        assert_eq!(model.id, "gpt-5.2");
        assert_eq!(model.provider, "openai");
        assert_eq!(model.api, "openai-completions");
    }

    #[test]
    fn test_model_metadata_anthropic() {
        let router = RouterProvider::new();
        let model = router.model_metadata("anthropic/claude-sonnet-4-20250514").unwrap();
        assert_eq!(model.id, "claude-sonnet-4-20250514");
        assert_eq!(model.provider, "anthropic");
        assert_eq!(model.api, "anthropic-messages");
    }

    #[test]
    fn test_model_metadata_openrouter_underlying() {
        let router = RouterProvider::new();
        let model = router
            .model_metadata("openrouter/anthropic/claude-sonnet-4-20250514")
            .unwrap();
        assert_eq!(model.id, "claude-sonnet-4-20250514");
        assert_eq!(model.provider, "anthropic");
        assert_eq!(model.api, "anthropic-messages");
    }

    #[test]
    fn test_model_metadata_openrouter_fallback() {
        let router = RouterProvider::new();
        // 不在注册表中的 openrouter 模型应 fallback
        let model = router
            .model_metadata("openrouter/openai/unknown-model-xyz")
            .unwrap();
        // model_id 保留 openrouter 的完整路径（含 underlying provider 前缀）
        assert_eq!(model.id, "openai/unknown-model-xyz");
        assert_eq!(model.provider, "openrouter");
        assert_eq!(model.api, "openai-completions");
    }

    #[test]
    fn test_model_metadata_unknown() {
        let router = RouterProvider::new();
        assert!(router.model_metadata("unknown/model").is_none());
    }

    #[test]
    fn test_cache_reuse() {
        let router = RouterProvider::new();
        let p1 = router.get_or_create_provider("openai", "https://api.openai.com/v1/chat/completions").unwrap();
        let p2 = router.get_or_create_provider("openai", "https://api.openai.com/v1/chat/completions").unwrap();
        assert!(Arc::ptr_eq(&p1, &p2));
    }

    #[test]
    fn test_cache_rebuild_on_base_url_change() {
        let router = RouterProvider::new();
        let p1 = router.get_or_create_provider("openai", "https://api.openai.com/v1/chat/completions").unwrap();
        let p2 = router.get_or_create_provider("openai", "https://proxy.example.com/v1/chat/completions").unwrap();
        assert!(!Arc::ptr_eq(&p1, &p2));
    }

    #[test]
    fn test_cache_different_providers() {
        let router = RouterProvider::new();
        let p1 = router.get_or_create_provider("openai", "https://api.openai.com/v1/chat/completions").unwrap();
        let p2 = router.get_or_create_provider("anthropic", "https://api.anthropic.com/v1/messages").unwrap();
        assert!(!Arc::ptr_eq(&p1, &p2));
    }

    #[test]
    fn test_get_or_create_openrouter() {
        let router = RouterProvider::new();
        let p = router.get_or_create_provider("openrouter", "https://openrouter.ai/api/v1/chat/completions").unwrap();
        assert_eq!(p.provider_name(), "openrouter");
    }

    #[test]
    fn test_get_or_create_ollama() {
        let router = RouterProvider::new();
        let p = router.get_or_create_provider("ollama", "http://localhost:11434/v1/chat/completions").unwrap();
        assert_eq!(p.provider_name(), "ollama");
    }

    #[test]
    fn test_get_or_create_unknown_provider() {
        let router = RouterProvider::new();
        let result = router.get_or_create_provider("unknown", "https://example.com");
        assert!(matches!(result, Err(LlmError::UnknownProvider(_))));
    }
}
