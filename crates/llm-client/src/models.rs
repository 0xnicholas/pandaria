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

pub struct ModelRegistry;

impl ModelRegistry {
    pub fn builtin() -> &'static Self {
        &ModelRegistry
    }

    pub fn get(&self, provider: &str, model_id: &str) -> Option<Model> {
        let key = format!("{}/{}", provider, model_id);
        models_data::MODELS.get(&key).cloned()
    }

    pub fn models_for_provider(&self, provider: &str) -> Vec<Model> {
        let pm = &*models_data::PROVIDER_MODELS;
        let models = &*models_data::MODELS;
        match pm.get(provider) {
            Some(ids) => ids
                .iter()
                .filter_map(|id| {
                    let key = format!("{}/{}", provider, id);
                    models.get(&key).cloned()
                })
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn providers(&self) -> Vec<String> {
        models_data::PROVIDER_MODELS.keys().cloned().collect()
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
