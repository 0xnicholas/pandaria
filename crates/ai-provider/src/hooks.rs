use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::models::Model;

/// HTTP response metadata (delivered before body consumption).
#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
}

/// Pre-request payload hook. Invoked before sending the HTTP request.
/// Returns true if the payload was modified.
pub type OnPayloadFn = Arc<
    dyn Fn(&mut serde_json::Value, &Model) -> Pin<Box<dyn Future<Output = bool> + Send>>
        + Send
        + Sync,
>;

/// Post-response hook. Invoked after HTTP response arrives, before
/// consuming the stream body. For observability/audit only.
pub type OnResponseFn = Arc<
    dyn Fn(&ProviderResponse, &Model) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Model;

    #[tokio::test]
    async fn test_on_payload_fn_callback() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = called.clone();

        let hook: OnPayloadFn =
            Arc::new(move |_payload: &mut serde_json::Value, _model: &Model| {
                let called = called_clone.clone();
                Box::pin(async move {
                    called.store(true, std::sync::atomic::Ordering::SeqCst);
                    true
                })
            });

        let mut payload = serde_json::json!({"key": "value"});
        let model = Model {
            id: "test".to_string(),
            name: "Test".to_string(),
            api: "test".to_string(),
            provider: "test".to_string(),
            base_url: "https://test.com".to_string(),
            reasoning: false,
            input_modalities: vec![],
            cost: crate::models::TokenCost::default(),
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: crate::models::ModelCompat::None,
        };

        let modified = hook(&mut payload, &model).await;
        assert!(modified);
        assert!(called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_on_response_fn_callback() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = called.clone();

        let hook: OnResponseFn = Arc::new(move |_response: &ProviderResponse, _model: &Model| {
            let called = called_clone.clone();
            Box::pin(async move {
                called.store(true, std::sync::atomic::Ordering::SeqCst);
            })
        });

        let response = ProviderResponse {
            status: 200,
            headers: std::collections::HashMap::new(),
        };
        let model = Model {
            id: "test".to_string(),
            name: "Test".to_string(),
            api: "test".to_string(),
            provider: "test".to_string(),
            base_url: "https://test.com".to_string(),
            reasoning: false,
            input_modalities: vec![],
            cost: crate::models::TokenCost::default(),
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: crate::models::ModelCompat::None,
        };

        hook(&response, &model).await;
        assert!(called.load(std::sync::atomic::Ordering::SeqCst));
    }
}
