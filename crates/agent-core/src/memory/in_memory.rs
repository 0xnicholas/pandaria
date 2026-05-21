use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use super::store::{MemoryError, MemoryStore};
use super::types::{MemoryContext, MemoryFact, MemoryQuery};

/// Pure in-memory implementation of `MemoryStore` for testing.
pub struct InMemoryStore {
    data: Mutex<HashMap<String, Vec<MemoryFact>>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn remember(&self, ctx: &MemoryContext, facts: &[MemoryFact]) -> Result<(), MemoryError> {
        let mut data = self.data.lock().unwrap();
        let key = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        data.entry(key).or_default().extend(facts.iter().cloned());
        Ok(())
    }

    async fn recall(&self, ctx: &MemoryContext, query: &MemoryQuery) -> Result<Vec<MemoryFact>, MemoryError> {
        let data = self.data.lock().unwrap();
        let prefix = if query.session_only {
            format!("{}:{}", ctx.tenant_id, ctx.session_id)
        } else {
            format!("{}:", ctx.tenant_id)
        };
        let results: Vec<MemoryFact> = data
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .flat_map(|(_, v)| v.clone())
            .filter(|f| f.content.contains(&query.text))
            .take(query.limit)
            .collect();
        Ok(results)
    }

    async fn forget_session(&self, ctx: &MemoryContext) -> Result<(), MemoryError> {
        let mut data = self.data.lock().unwrap();
        let key = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        data.remove(&key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_in_memory_store_crud() {
        let store = InMemoryStore::new();
        let ctx = MemoryContext {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            user_id: None,
        };

        store.remember(&ctx, &[MemoryFact {
            id: None,
            content: "hello".to_string(),
            category: None,
            importance: None,
            metadata: serde_json::Value::Null,
        }]).await.unwrap();

        let results = store.recall(&ctx, &MemoryQuery {
            text: "hello".to_string(),
            limit: 5,
            session_only: false,
        }).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "hello");

        store.forget_session(&ctx).await.unwrap();
        let results = store.recall(&ctx, &MemoryQuery {
            text: "hello".to_string(),
            limit: 5,
            session_only: false,
        }).await.unwrap();
        assert!(results.is_empty());
    }
}
