//! Pure in-memory implementation of `MemoryStore` for testing.

use std::collections::HashMap;
use std::time::SystemTime;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::store::{MemoryError, MemoryStore};
use super::types::MemoryContext;

struct MemoryRecord {
    content: String,
    metadata: serde_json::Value,
    timestamp: SystemTime,
}

/// Pure in-memory implementation of `MemoryStore` for testing.
pub struct InMemoryStore {
    data: RwLock<HashMap<String, Vec<MemoryRecord>>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn remember(
        &self,
        ctx: &MemoryContext,
        content: &str,
        metadata: &serde_json::Value,
    ) -> Result<(), MemoryError> {
        let key = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        self.data.write().await.entry(key).or_default().push(MemoryRecord {
            content: content.to_string(),
            metadata: metadata.clone(),
            timestamp: SystemTime::now(),
        });
        Ok(())
    }

    async fn recall(&self, ctx: &MemoryContext, query: &str) -> Result<Vec<String>, MemoryError> {
        let prefix = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        let results: Vec<String> = self
            .data
            .read()
            .await
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .flat_map(|(_, records)| records.iter())
            .filter(|r| r.content.contains(query))
            .map(|r| r.content.clone())
            .take(5)
            .collect();
        Ok(results)
    }

    async fn forget_session(&self, ctx: &MemoryContext) -> Result<(), MemoryError> {
        let key = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        self.data.write().await.remove(&key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> MemoryContext {
        MemoryContext {
            tenant_id: "t1".into(),
            session_id: "s1".into(),
            user_id: None,
            model: "gpt-4".into(),
            session_started_at: SystemTime::now(),
        }
    }

    #[tokio::test]
    async fn test_remember_and_recall() {
        let store = InMemoryStore::new();
        let ctx = make_ctx();

        store
            .remember(&ctx, "User likes TypeScript", &serde_json::json!({"turn": 1}))
            .await
            .unwrap();

        let results = store.recall(&ctx, "TypeScript").await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("TypeScript"));
    }

    #[tokio::test]
    async fn test_forget_session() {
        let store = InMemoryStore::new();
        let ctx = make_ctx();

        store
            .remember(&ctx, "test", &serde_json::json!({}))
            .await
            .unwrap();
        store.forget_session(&ctx).await.unwrap();

        let results = store.recall(&ctx, "test").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_tenant_isolation() {
        let store = InMemoryStore::new();
        let ctx_a = MemoryContext {
            tenant_id: "ta".into(),
            session_id: "s1".into(),
            user_id: None,
            model: "gpt-4".into(),
            session_started_at: SystemTime::now(),
        };
        let ctx_b = MemoryContext {
            tenant_id: "tb".into(),
            session_id: "s1".into(),
            user_id: None,
            model: "gpt-4".into(),
            session_started_at: SystemTime::now(),
        };

        store
            .remember(&ctx_a, "secret-a", &serde_json::json!({}))
            .await
            .unwrap();
        store
            .remember(&ctx_b, "secret-b", &serde_json::json!({}))
            .await
            .unwrap();

        let ra = store.recall(&ctx_a, "secret").await.unwrap();
        assert_eq!(ra.len(), 1);
        assert!(ra[0].contains("secret-a"));

        let rb = store.recall(&ctx_b, "secret").await.unwrap();
        assert_eq!(rb.len(), 1);
        assert!(rb[0].contains("secret-b"));
    }
}
