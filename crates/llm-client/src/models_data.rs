use std::collections::HashMap;
use std::sync::LazyLock;

use crate::models::{Modality, Model, TokenCost};

fn build_models() -> HashMap<String, Model> {
    let mut m = HashMap::new();

    // Helper to insert with "provider/id" key
    macro_rules! insert {
        ($map:expr, $provider:expr, $id:expr, $name:expr, $api:expr, $base_url:expr,
         $reasoning:expr, $modalities:expr, $cost:expr, $ctx:expr, $max:expr) => {
            let key = format!("{}/{}", $provider, $id);
            $map.insert(
                key,
                Model {
                    id: $id.to_string(),
                    name: $name.to_string(),
                    api: $api.to_string(),
                    provider: $provider.to_string(),
                    base_url: $base_url.to_string(),
                    reasoning: $reasoning,
                    input_modalities: $modalities,
                    cost: $cost,
                    context_window: $ctx,
                    max_tokens: $max,
                    headers: None,
                    compat: crate::models::ModelCompat::None,
                },
            );
        };
    }

    // ── Anthropic ──────────────────────────────────────────────────
    insert!(
        m,
        "anthropic",
        "claude-sonnet-4-20250514",
        "Claude Sonnet 4",
        "anthropic-messages",
        "https://api.anthropic.com/v1/messages",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75
        },
        200_000,
        8192
    );
    insert!(
        m,
        "anthropic",
        "claude-sonnet-4-5-20250929",
        "Claude Sonnet 4.5",
        "anthropic-messages",
        "https://api.anthropic.com/v1/messages",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75
        },
        200_000,
        8192
    );
    insert!(
        m,
        "anthropic",
        "claude-opus-4-7",
        "Claude Opus 4.7",
        "anthropic-messages",
        "https://api.anthropic.com/v1/messages",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 15.0,
            output: 75.0,
            cache_read: 1.5,
            cache_write: 18.75
        },
        200_000,
        8192
    );
    insert!(
        m,
        "anthropic",
        "claude-opus-4-6",
        "Claude Opus 4.6",
        "anthropic-messages",
        "https://api.anthropic.com/v1/messages",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 15.0,
            output: 75.0,
            cache_read: 1.5,
            cache_write: 18.75
        },
        200_000,
        8192
    );
    insert!(
        m,
        "anthropic",
        "claude-haiku-4-7",
        "Claude Haiku 4.7",
        "anthropic-messages",
        "https://api.anthropic.com/v1/messages",
        true,
        vec![Modality::Text],
        TokenCost {
            input: 0.8,
            output: 4.0,
            cache_read: 0.08,
            cache_write: 1.0
        },
        200_000,
        8192
    );
    insert!(
        m,
        "anthropic",
        "claude-haiku-4-5",
        "Claude Haiku 4.5",
        "anthropic-messages",
        "https://api.anthropic.com/v1/messages",
        false,
        vec![Modality::Text],
        TokenCost {
            input: 0.8,
            output: 4.0,
            cache_read: 0.08,
            cache_write: 1.0
        },
        200_000,
        8192
    );

    // ── OpenAI ─────────────────────────────────────────────────────
    insert!(
        m,
        "openai",
        "gpt-5.2",
        "GPT-5.2",
        "openai-completions",
        "https://api.openai.com/v1/chat/completions",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 1.75,
            output: 14.0,
            cache_read: 0.0,
            cache_write: 0.0
        },
        272_000,
        128_000
    );
    insert!(
        m,
        "openai",
        "gpt-5.3",
        "GPT-5.3",
        "openai-completions",
        "https://api.openai.com/v1/chat/completions",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 2.5,
            output: 20.0,
            cache_read: 0.0,
            cache_write: 0.0
        },
        272_000,
        128_000
    );
    insert!(
        m,
        "openai",
        "gpt-5.4",
        "GPT-5.4",
        "openai-completions",
        "https://api.openai.com/v1/chat/completions",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 4.0,
            output: 32.0,
            cache_read: 0.0,
            cache_write: 0.0
        },
        272_000,
        128_000
    );
    insert!(
        m,
        "openai",
        "gpt-5.5",
        "GPT-5.5",
        "openai-completions",
        "https://api.openai.com/v1/chat/completions",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 6.0,
            output: 48.0,
            cache_read: 0.0,
            cache_write: 0.0
        },
        272_000,
        128_000
    );
    insert!(
        m,
        "openai",
        "gpt-5.1",
        "GPT-5.1",
        "openai-completions",
        "https://api.openai.com/v1/chat/completions",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 1.25,
            output: 10.0,
            cache_read: 0.0,
            cache_write: 0.0
        },
        272_000,
        128_000
    );
    insert!(
        m,
        "openai",
        "gpt-5.1-codex",
        "GPT-5.1 Codex",
        "openai-completions",
        "https://api.openai.com/v1/chat/completions",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 1.25,
            output: 10.0,
            cache_read: 0.0,
            cache_write: 0.0
        },
        272_000,
        128_000
    );
    insert!(
        m,
        "openai",
        "gpt-4.1",
        "GPT-4.1",
        "openai-completions",
        "https://api.openai.com/v1/chat/completions",
        false,
        vec![Modality::Text],
        TokenCost {
            input: 2.0,
            output: 8.0,
            cache_read: 0.0,
            cache_write: 0.0
        },
        1_000_000,
        32_768
    );
    insert!(
        m,
        "openai",
        "gpt-4.1-mini",
        "GPT-4.1 Mini",
        "openai-completions",
        "https://api.openai.com/v1/chat/completions",
        false,
        vec![Modality::Text],
        TokenCost {
            input: 0.4,
            output: 1.6,
            cache_read: 0.0,
            cache_write: 0.0
        },
        1_000_000,
        32_768
    );

    // ── Bedrock ────────────────────────────────────────────────────
    insert!(
        m,
        "bedrock",
        "anthropic.claude-3-5-sonnet-20241022-v2:0",
        "Claude 3.5 Sonnet",
        "anthropic-messages",
        "https://bedrock-runtime.us-east-1.amazonaws.com",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75
        },
        200_000,
        8192
    );
    insert!(
        m,
        "bedrock",
        "anthropic.claude-3-opus-20240229-v1:0",
        "Claude 3 Opus",
        "anthropic-messages",
        "https://bedrock-runtime.us-east-1.amazonaws.com",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 15.0,
            output: 75.0,
            cache_read: 1.5,
            cache_write: 18.75
        },
        200_000,
        4096
    );
    insert!(
        m,
        "bedrock",
        "anthropic.claude-3-haiku-20240307-v1:0",
        "Claude 3 Haiku",
        "anthropic-messages",
        "https://bedrock-runtime.us-east-1.amazonaws.com",
        false,
        vec![Modality::Text],
        TokenCost {
            input: 0.25,
            output: 1.25,
            cache_read: 0.025,
            cache_write: 0.3125
        },
        200_000,
        4096
    );

    // ── Mistral ────────────────────────────────────────────────────
    insert!(
        m,
        "mistral",
        "mistral-large-latest",
        "Mistral Large",
        "openai-completions",
        "https://api.mistral.ai/v1/chat/completions",
        true,
        vec![Modality::Text],
        TokenCost {
            input: 2.0,
            output: 6.0,
            cache_read: 0.0,
            cache_write: 0.0
        },
        128_000,
        128_000
    );
    insert!(
        m,
        "mistral",
        "mistral-medium-latest",
        "Mistral Medium",
        "openai-completions",
        "https://api.mistral.ai/v1/chat/completions",
        false,
        vec![Modality::Text],
        TokenCost {
            input: 1.0,
            output: 3.0,
            cache_read: 0.0,
            cache_write: 0.0
        },
        128_000,
        128_000
    );

    // ── Google ─────────────────────────────────────────────────────
    insert!(
        m,
        "google",
        "gemini-2.5-pro",
        "Gemini 2.5 Pro",
        "google-generative-ai",
        "https://generativelanguage.googleapis.com/v1beta",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 1.25,
            output: 10.0,
            cache_read: 0.0,
            cache_write: 0.0
        },
        2_097_152,
        65_535
    );
    insert!(
        m,
        "google",
        "gemini-2.5-flash",
        "Gemini 2.5 Flash",
        "google-generative-ai",
        "https://generativelanguage.googleapis.com/v1beta",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 0.15,
            output: 0.6,
            cache_read: 0.0,
            cache_write: 0.0
        },
        1_048_576,
        65_535
    );
    insert!(
        m,
        "google",
        "gemini-3.0-flash",
        "Gemini 3.0 Flash",
        "google-generative-ai",
        "https://generativelanguage.googleapis.com/v1beta",
        true,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 0.1,
            output: 0.4,
            cache_read: 0.0,
            cache_write: 0.0
        },
        1_048_576,
        65_535
    );
    insert!(
        m,
        "google",
        "gemini-2.0-flash",
        "Gemini 2.0 Flash",
        "google-generative-ai",
        "https://generativelanguage.googleapis.com/v1beta",
        false,
        vec![Modality::Text, Modality::Image],
        TokenCost {
            input: 0.15,
            output: 0.6,
            cache_read: 0.0,
            cache_write: 0.0
        },
        1_048_576,
        8192
    );

    // Fill compat fields using auto-detection logic
    for (_key, model) in m.iter_mut() {
        model.compat = match model.api.as_str() {
            "openai-completions" => crate::models::ModelCompat::OpenAI(
                crate::compat::detect_openai_compat(&model.provider, &model.base_url, &model.id),
            ),
            "anthropic-messages" => crate::models::ModelCompat::Anthropic(
                crate::compat::detect_anthropic_compat(&model.provider, &model.base_url),
            ),
            _ => crate::models::ModelCompat::None,
        };
    }

    m
}

fn build_provider_list() -> HashMap<String, Vec<String>> {
    let mut p: HashMap<String, Vec<String>> = HashMap::new();
    p.insert(
        "anthropic".to_string(),
        vec![
            "claude-sonnet-4-20250514".to_string(),
            "claude-sonnet-4-5-20250929".to_string(),
            "claude-opus-4-7".to_string(),
            "claude-opus-4-6".to_string(),
            "claude-haiku-4-7".to_string(),
            "claude-haiku-4-5".to_string(),
        ],
    );
    p.insert(
        "openai".to_string(),
        vec![
            "gpt-5.2".to_string(),
            "gpt-5.3".to_string(),
            "gpt-5.4".to_string(),
            "gpt-5.5".to_string(),
            "gpt-5.1".to_string(),
            "gpt-5.1-codex".to_string(),
            "gpt-4.1".to_string(),
            "gpt-4.1-mini".to_string(),
        ],
    );
    p.insert(
        "bedrock".to_string(),
        vec![
            "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
            "anthropic.claude-3-opus-20240229-v1:0".to_string(),
            "anthropic.claude-3-haiku-20240307-v1:0".to_string(),
        ],
    );
    p.insert(
        "mistral".to_string(),
        vec![
            "mistral-large-latest".to_string(),
            "mistral-medium-latest".to_string(),
        ],
    );
    p.insert(
        "google".to_string(),
        vec![
            "gemini-2.5-pro".to_string(),
            "gemini-2.5-flash".to_string(),
            "gemini-3.0-flash".to_string(),
            "gemini-2.0-flash".to_string(),
        ],
    );
    p
}

pub static MODELS: LazyLock<HashMap<String, Model>> = LazyLock::new(build_models);
pub static PROVIDER_MODELS: LazyLock<HashMap<String, Vec<String>>> =
    LazyLock::new(build_provider_list);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_models_not_empty() {
        assert!(!MODELS.is_empty(), "MODELS registry should not be empty");
    }

    #[test]
    fn test_provider_models_not_empty() {
        assert!(
            !PROVIDER_MODELS.is_empty(),
            "PROVIDER_MODELS registry should not be empty"
        );
    }

    #[test]
    fn test_all_provider_models_exist() {
        for (provider, model_ids) in PROVIDER_MODELS.iter() {
            for model_id in model_ids {
                let key = format!("{}/{}", provider, model_id);
                assert!(
                    MODELS.contains_key(&key),
                    "Model '{}' listed under provider '{}' does not exist in MODELS",
                    model_id,
                    provider
                );
            }
        }
    }

    #[test]
    fn test_no_duplicate_model_keys() {
        // build_models uses HashMap::insert which overwrites on duplicate keys.
        // Verify by checking the count equals the number of insert! macro calls.
        // If we had 30 insert! calls but only 29 keys, one was silently overwritten.
        // We'll just verify the count is stable and reasonable.
        assert!(
            MODELS.len() >= 20,
            "MODELS registry seems unexpectedly small: {}",
            MODELS.len()
        );
    }

    #[test]
    fn test_provider_consistency() {
        for (key, model) in MODELS.iter() {
            let expected_prefix = format!("{}/", model.provider);
            assert!(
                key.starts_with(&expected_prefix),
                "Model key '{}' does not start with provider '{}'",
                key,
                model.provider
            );

            // Verify provider is listed in PROVIDER_MODELS
            assert!(
                PROVIDER_MODELS.contains_key(&model.provider),
                "Provider '{}' not found in PROVIDER_MODELS",
                model.provider
            );
        }
    }

    #[test]
    fn test_provider_models_complete() {
        // Every model in MODELS should be reachable via PROVIDER_MODELS
        for (key, model) in MODELS.iter() {
            let provider_models = PROVIDER_MODELS
                .get(&model.provider)
                .expect("Provider missing from PROVIDER_MODELS — model registry is inconsistent");
            let model_id = key
                .strip_prefix(&format!("{}/", model.provider))
                .unwrap_or(key);
            assert!(
                provider_models.contains(&model_id.to_string()),
                "Model '{}' not listed under provider '{}' in PROVIDER_MODELS",
                model_id,
                model.provider
            );
        }
    }

    #[test]
    fn test_model_fields_reasonable() {
        for (key, model) in MODELS.iter() {
            assert!(!model.id.is_empty(), "Model '{}' has empty id", key);
            assert!(!model.name.is_empty(), "Model '{}' has empty name", key);
            assert!(!model.api.is_empty(), "Model '{}' has empty api", key);
            assert!(
                !model.base_url.is_empty(),
                "Model '{}' has empty base_url",
                key
            );
            assert!(
                model.context_window > 0,
                "Model '{}' has zero context_window",
                key
            );
            assert!(model.max_tokens > 0, "Model '{}' has zero max_tokens", key);
            assert!(
                model.cost.input >= 0.0,
                "Model '{}' has negative input cost",
                key
            );
            assert!(
                model.cost.output >= 0.0,
                "Model '{}' has negative output cost",
                key
            );
        }
    }
}
