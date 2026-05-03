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
