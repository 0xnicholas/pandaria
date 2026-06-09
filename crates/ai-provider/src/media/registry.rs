use crate::media::MediaTaskType;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct MediaModel {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub base_url: String,
    pub supported_tasks: Vec<MediaTaskType>,
    pub cost_per_call: Option<f64>,
    pub headers: Option<HashMap<String, String>>,
}

pub struct MediaModelRegistry {
    models: HashMap<String, MediaModel>,
}

impl Default for MediaModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaModelRegistry {
    pub fn new() -> Self {
        Self {
            models: HashMap::new(),
        }
    }

    pub fn insert(&mut self, model: MediaModel) {
        self.models.insert(model.id.clone(), model);
    }

    pub fn get(&self, model_id: &str) -> Option<&MediaModel> {
        self.models.get(model_id)
    }

    pub fn models_for_provider(&self, provider: &str) -> Vec<&MediaModel> {
        self.models
            .values()
            .filter(|m| m.provider == provider)
            .collect()
    }

    pub fn build_default() -> Self {
        let mut registry = Self::new();

        registry.insert(MediaModel {
            id: "doubao-seedream-5-0".to_string(),
            name: "Doubao Seedream 5.0".to_string(),
            provider: "doubao".to_string(),
            base_url: "https://ark.cn-beijing.volces.com/api/v3".to_string(),
            supported_tasks: vec![MediaTaskType::ImageGeneration],
            cost_per_call: Some(0.2),
            headers: None,
        });

        registry.insert(MediaModel {
            id: "doubao-seedance-2-0".to_string(),
            name: "Doubao Seedance 2.0".to_string(),
            provider: "doubao".to_string(),
            base_url: "https://ark.cn-beijing.volces.com/api/v3".to_string(),
            supported_tasks: vec![MediaTaskType::VideoGeneration],
            cost_per_call: Some(1.5),
            headers: None,
        });

        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_model_lookup() {
        let registry = MediaModelRegistry::build_default();
        assert!(registry.get("doubao-seedream-5-0").is_some());
        assert!(registry.get("doubao-seedance-2-0").is_some());
        assert!(registry.get("nonexistent").is_none());
    }
}
