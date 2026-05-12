use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Input modality.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Modality {
    Text,
    Image,
}

/// Cost per million tokens (USD).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct TokenCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// Static model metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: String,
    pub provider: String,
    pub base_url: String,
    pub reasoning: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub input_modalities: Vec<Modality>,
    pub cost: TokenCost,
    pub context_window: u32,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    pub compat: ModelCompat,
}

/// Per-API compatibility flags, tag-discriminated by api field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "api")]
#[allow(clippy::large_enum_variant)]
pub enum ModelCompat {
    #[serde(rename = "openai-completions")]
    OpenAI(crate::compat::OpenAiCompat),
    #[serde(rename = "anthropic-messages")]
    Anthropic(crate::compat::AnthropicCompat),
    #[serde(other)]
    None,
}

// compat field deferred to Phase 6 (T6.2)

pub fn calculate_cost(model: &Model, usage: &crate::Usage) -> TokenCost {
    let input = (model.cost.input / 1_000_000.0) * usage.input_tokens as f64;
    let output = (model.cost.output / 1_000_000.0) * usage.output_tokens as f64;
    let cache_read =
        (model.cost.cache_read / 1_000_000.0) * usage.cache_read_input_tokens.unwrap_or(0) as f64;
    let cache_write = (model.cost.cache_write / 1_000_000.0)
        * usage.cache_creation_input_tokens.unwrap_or(0) as f64;
    TokenCost {
        input,
        output,
        cache_read,
        cache_write,
    }
}

pub fn models_are_equal(a: Option<&Model>, b: Option<&Model>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.id == b.id && a.provider == b.provider,
        _ => false,
    }
}

pub fn supports_xhigh(model_id: &str) -> bool {
    if model_id.contains("gpt-5.2")
        || model_id.contains("gpt-5.3")
        || model_id.contains("gpt-5.4")
        || model_id.contains("gpt-5.5")
        || model_id.contains("deepseek-v4-pro")
    {
        return true;
    }
    if model_id.contains("opus-4-6")
        || model_id.contains("opus-4.6")
        || model_id.contains("opus-4-7")
        || model_id.contains("opus-4.7")
    {
        return true;
    }
    false
}

// ── ModelRegistry ──

use crate::models_data;

/// Model registry with optional custom model overrides.
///
/// Custom models registered via [`register`](ModelRegistry::register) take
/// precedence over builtin models with the same `provider/model_id` key.
pub struct ModelRegistry {
    custom: HashMap<String, Model>,
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelRegistry {
    /// Create a new registry with no custom models.
    pub fn new() -> Self {
        Self {
            custom: HashMap::new(),
        }
    }

    /// Create a registry pre-loaded from builtin models only.
    pub fn builtin() -> Self {
        Self::new()
    }

    /// Register a custom model. Custom models override builtin models
    /// with the same provider/model_id key.
    pub fn register(&mut self, model: Model) {
        let key = format!("{}/{}", model.provider, model.id);
        self.custom.insert(key, model);
    }

    pub fn get(&self, provider: &str, model_id: &str) -> Option<Model> {
        let key = format!("{}/{}", provider, model_id);
        self.custom
            .get(&key)
            .cloned()
            .or_else(|| models_data::MODELS.get(&key).cloned())
    }

    pub fn models_for_provider(&self, provider: &str) -> Vec<Model> {
        let prefix = format!("{}/", provider);
        let mut seen: HashMap<String, bool> = HashMap::new();
        let mut result: Vec<Model> = Vec::new();

        // Custom models first
        for (k, m) in &self.custom {
            if k.starts_with(&prefix) {
                seen.insert(m.id.clone(), true);
                result.push(m.clone());
            }
        }

        // Builtin models (skip keys already present in custom)
        if let Some(ids) = models_data::PROVIDER_MODELS.get(provider) {
            for id in ids.iter() {
                if !seen.contains_key(id) {
                    let key = format!("{}/{}", provider, id);
                    if let Some(m) = models_data::MODELS.get(&key).cloned() {
                        result.push(m);
                    }
                }
            }
        }

        result
    }

    pub fn providers(&self) -> Vec<String> {
        let mut p: Vec<String> = models_data::PROVIDER_MODELS.keys().cloned().collect();
        for k in self.custom.keys() {
            if let Some(prov) = k.split('/').next()
                && !p.iter().any(|x| x == prov)
            {
                p.push(prov.to_string());
            }
        }
        p
    }
}

pub fn get_model(provider: &str, model_id: &str) -> Option<Model> {
    ModelRegistry::builtin().get(provider, model_id)
}

pub fn models_for_provider(provider: &str) -> Vec<Model> {
    ModelRegistry::builtin().models_for_provider(provider)
}

pub fn providers() -> Vec<String> {
    ModelRegistry::builtin().providers()
}

/// Return just the model IDs (strings) for a given provider.
/// Used by provider `models()` implementations.
pub fn models_for_provider_names(provider: &str) -> Vec<String> {
    models_for_provider(provider)
        .into_iter()
        .map(|m| m.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_model_found() {
        let m = get_model("anthropic", "claude-sonnet-4-20250514");
        assert!(m.is_some());
        assert_eq!(m.unwrap().id, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_get_model_not_found() {
        assert!(get_model("openai", "nonexistent").is_none());
    }

    #[test]
    fn test_models_for_provider() {
        let models = models_for_provider("openai");
        assert!(models.len() >= 6);
    }

    #[test]
    fn test_providers_list() {
        let p = providers();
        assert!(p.iter().any(|s| s == "anthropic"));
        assert!(p.iter().any(|s| s == "openai"));
        assert!(p.iter().any(|s| s == "google"));
    }

    #[test]
    fn test_calculate_cost() {
        let model = get_model("anthropic", "claude-sonnet-4-20250514").unwrap();
        let usage = crate::Usage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_read_input_tokens: Some(100_000),
            cache_creation_input_tokens: Some(50_000),
            total_tokens: 1_650_000,
        };
        let cost = calculate_cost(&model, &usage);
        assert!((cost.input - 3.0).abs() < 0.01);
        assert!((cost.output - 7.5).abs() < 0.01);
    }

    #[test]
    fn test_supports_xhigh() {
        assert!(supports_xhigh("gpt-5.2"));
        assert!(supports_xhigh("gpt-5.5"));
        assert!(supports_xhigh("opus-4-7"));
        assert!(supports_xhigh("deepseek-v4-pro"));
        assert!(!supports_xhigh("gpt-4.1"));
        assert!(!supports_xhigh("claude-sonnet-4-20250514"));
    }

    #[test]
    fn test_models_are_equal() {
        let a = get_model("openai", "gpt-5.2");
        let b = get_model("openai", "gpt-5.2");
        assert!(models_are_equal(a.as_ref(), b.as_ref()));
        let c = get_model("openai", "gpt-5.3");
        assert!(!models_are_equal(a.as_ref(), c.as_ref()));
        assert!(!models_are_equal(None, a.as_ref()));
        assert!(!models_are_equal(a.as_ref(), None));
    }

    #[test]
    fn test_get_model_missing_provider() {
        assert!(get_model("nonexistent", "any-model").is_none());
        assert!(models_for_provider("nonexistent").is_empty());
    }
}
