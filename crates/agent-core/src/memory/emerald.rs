//! HTTP adapter bridging Pandaria's `MemoryStore` trait to Emerald REST API.
//!
//! All Pandaria-specific logic lives here. Emerald receives generic HTTP calls
//! with no knowledge of the caller's runtime.
//!
//! See `docs/specs/2026-05-27-pandaria-emerald-memorystore.md` for the
//! upstream interface contract.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use tracing::{error, warn};

use super::store::{MemoryError, MemoryStore};
use super::types::MemoryContext;

/// HTTP adapter for Emerald memory system.
///
/// Implements `MemoryStore` by calling Emerald's REST endpoints:
/// - `POST /v1/memories` for `remember`
/// - `POST /v1/search` for `recall`
pub struct EmeraldMemoryStore {
    client: Client,
    base_url: String,
    api_key: String,
}

impl EmeraldMemoryStore {
    /// Create a new `EmeraldMemoryStore`.
    ///
    /// # Arguments
    /// * `base_url` — Emerald base URL (e.g. `http://localhost:8000`)
    /// * `api_key` — API key for Bearer token authorization
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
    }

    /// Entity mapping: `tenant_id` is the cross-session identity in Emerald.
    fn entity_id(&self, ctx: &MemoryContext) -> String {
        ctx.tenant_id.clone()
    }
}

#[async_trait]
impl MemoryStore for EmeraldMemoryStore {
    /// Send turn content to Emerald.
    ///
    /// Merges Pandaria session context into metadata so Emerald stores it
    /// verbatim without interpretation.
    async fn remember(
        &self,
        ctx: &MemoryContext,
        content: &str,
        metadata: &Value,
    ) -> Result<(), MemoryError> {
        let entity_id = self.entity_id(ctx);
        let url = format!("{}/v1/memories", self.base_url);

        let mut meta = metadata.clone();
        if let Some(obj) = meta.as_object_mut() {
            obj.insert("session_id".to_string(), serde_json::json!(ctx.session_id));
            obj.insert("model".to_string(), serde_json::json!(ctx.model));
        }

        let body = serde_json::json!({
            "content": content,
            "entity_id": entity_id,
            "content_type": "conversation",
            "metadata": meta,
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| MemoryError::StoreError(format!("HTTP error: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            error!(
                status = %status,
                body = %text,
                "EmeraldMemoryStore::remember failed"
            );
            return Err(MemoryError::StoreError(format!(
                "Emerald error ({}): {}",
                status, text
            )));
        }

        Ok(())
    }

    /// Retrieve relevant memories from Emerald.
    ///
    /// Returns a `Vec<String>` of memory content texts, ready to be injected
    /// into the LLM context window.
    async fn recall(
        &self,
        ctx: &MemoryContext,
        query: &str,
    ) -> Result<Vec<String>, MemoryError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }

        let entity_id = self.entity_id(ctx);
        let url = format!("{}/v1/search", self.base_url);

        let body = serde_json::json!({
            "q": query,
            "entity_id": entity_id,
            "search_mode": "hybrid",
            "top_k": 5,
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map_err(|e| MemoryError::StoreError(format!("HTTP error: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            warn!(
                status = %status,
                body = %text,
                "EmeraldMemoryStore::recall failed"
            );
            return Err(MemoryError::StoreError(format!(
                "Emerald error ({}): {}",
                status, text
            )));
        }

        let data: Value = response
            .json()
            .await
            .map_err(|e| MemoryError::StoreError(format!("JSON parse error: {}", e)))?;

        let results = data
            .get("data")
            .and_then(|d| d.get("results"))
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        item.get("content").and_then(|c| c.as_str()).map(String::from)
                    })
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();

        Ok(results)
    }

    /// v0.2.0: no-op.
    ///
    /// Emerald's auto-forgetting engine handles time-based expiration,
    /// but does NOT react to Pandaria session deletion.
    /// Future: call Emerald's explicit forget API when available.
    async fn forget_session(&self, _ctx: &MemoryContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_ctx() -> MemoryContext {
        MemoryContext {
            tenant_id: "tenant_1".into(),
            session_id: "sess_1".into(),
            user_id: None,
            model: "gpt-4".into(),
            session_started_at: SystemTime::now(),
        }
    }

    #[tokio::test]
    async fn test_remember_posts_correct_body() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/memories"))
            .and(body_json(serde_json::json!({
                "content": "**User**: hello\n\n**Assistant**: hi\n\n",
                "entity_id": "tenant_1",
                "content_type": "conversation",
                "metadata": {
                    "session_id": "sess_1",
                    "model": "gpt-4",
                    "foo": "bar"
                }
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": {
                        "memory_ids": ["abc123"],
                        "pipeline_status": "done",
                        "extracted_count": 1
                    },
                    "meta": { "request_id": "req-1", "took_ms": 42 }
                })),
            )
            .mount(&mock_server)
            .await;

        let store = EmeraldMemoryStore::new(&mock_server.uri(), "test_key");
        let ctx = make_ctx();

        store
            .remember(
                &ctx,
                "**User**: hello\n\n**Assistant**: hi\n\n",
                &serde_json::json!({"foo": "bar"}),
            )
            .await
            .expect("remember should succeed");
    }

    #[tokio::test]
    async fn test_recall_returns_content_list() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .and(body_json(serde_json::json!({
                "q": "hello",
                "entity_id": "tenant_1",
                "search_mode": "hybrid",
                "top_k": 5
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": {
                        "results": [
                            {
                                "id": "hex1",
                                "content": "hello world",
                                "score": 0.95,
                                "source": "memory",
                                "memory_type": "fact",
                                "is_latest": true
                            },
                            {
                                "id": "hex2",
                                "content": "hello rust",
                                "score": 0.85,
                                "source": "memory",
                                "memory_type": "fact",
                                "is_latest": true
                            }
                        ],
                        "search_mode": "hybrid"
                    }
                })),
            )
            .mount(&mock_server)
            .await;

        let store = EmeraldMemoryStore::new(&mock_server.uri(), "test_key");
        let ctx = make_ctx();

        let results = store.recall(&ctx, "hello").await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], "hello world");
        assert_eq!(results[1], "hello rust");
    }

    #[tokio::test]
    async fn test_recall_returns_err_on_server_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&mock_server)
            .await;

        let store = EmeraldMemoryStore::new(&mock_server.uri(), "test_key");
        let ctx = make_ctx();

        let result = store.recall(&ctx, "hello").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("500"));
        assert!(err.contains("Internal Server Error"));
    }

    #[tokio::test]
    async fn test_recall_returns_empty_on_malformed_json() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&mock_server)
            .await;

        let store = EmeraldMemoryStore::new(&mock_server.uri(), "test_key");
        let ctx = make_ctx();

        let result = store.recall(&ctx, "hello").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("JSON parse error"));
    }

    #[tokio::test]
    async fn test_recall_returns_empty_when_no_results_field() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": { "search_mode": "hybrid" }
                })),
            )
            .mount(&mock_server)
            .await;

        let store = EmeraldMemoryStore::new(&mock_server.uri(), "test_key");
        let ctx = make_ctx();

        let results = store.recall(&ctx, "hello").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_entity_id_uses_tenant_id_not_session_id() {
        // This test verifies the mapping strategy: entity_id = tenant_id.
        // Different sessions for the same tenant should use the same entity_id.
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .and(body_json(serde_json::json!({
                "q": "query",
                "entity_id": "shared_tenant",
                "search_mode": "hybrid",
                "top_k": 5
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "results": [] }
            })))
            .mount(&mock_server)
            .await;

        let store = EmeraldMemoryStore::new(&mock_server.uri(), "test_key");

        let ctx_a = MemoryContext {
            tenant_id: "shared_tenant".into(),
            session_id: "sess_a".into(),
            user_id: None,
            model: "gpt-4".into(),
            session_started_at: SystemTime::now(),
        };
        let ctx_b = MemoryContext {
            tenant_id: "shared_tenant".into(),
            session_id: "sess_b".into(),
            user_id: None,
            model: "gpt-4".into(),
            session_started_at: SystemTime::now(),
        };

        // Both calls should succeed because entity_id is the same (tenant_id).
        store.recall(&ctx_a, "query").await.unwrap();
        store.recall(&ctx_b, "query").await.unwrap();
    }

    #[tokio::test]
    async fn test_recall_empty_query_short_circuits() {
        let store = EmeraldMemoryStore::new("http://localhost:8000", "test_key");
        let ctx = make_ctx();

        // Empty or whitespace-only queries should return empty without hitting the server.
        let results = store.recall(&ctx, "").await.unwrap();
        assert!(results.is_empty());

        let results = store.recall(&ctx, "   ").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_forget_session_is_noop() {
        let store = EmeraldMemoryStore::new("http://localhost:8000", "test_key");
        let ctx = make_ctx();

        // Should not panic or error.
        store.forget_session(&ctx).await.unwrap();
    }
}
