use async_trait::async_trait;
use redis::{AsyncCommands, aio::MultiplexedConnection};
use tracing::debug;

use agent_core::{AgentError, SessionEntry, SessionStore};

/// Redis implementation of [`SessionStore`].
///
/// Stores each session's entries as a JSON-encoded value under the key
/// `pandaria:session:{tenant_id}:{session_id}`.
///
/// Session keys are set with a default TTL of 7 days to prevent unbounded
/// accumulation of abandoned sessions.
#[derive(Clone)]
pub struct RedisSessionStore {
    conn: MultiplexedConnection,
    ttl_seconds: u64,
}

impl RedisSessionStore {
    /// Default TTL for session keys (7 days).
    pub const DEFAULT_TTL_SECONDS: u64 = 604800;

    /// Create a new store backed by the given Redis multiplexed connection.
    ///
    /// ```ignore
    /// let client = redis::Client::open("redis://127.0.0.1/")?;
    /// let conn = client.get_multiplexed_async_connection().await?;
    /// let store = RedisSessionStore::new(conn);
    /// ```
    pub fn new(conn: MultiplexedConnection) -> Self {
        Self {
            conn,
            ttl_seconds: Self::DEFAULT_TTL_SECONDS,
        }
    }

    /// Set a custom TTL for session keys.
    pub fn with_ttl(mut self, ttl_seconds: u64) -> Self {
        self.ttl_seconds = ttl_seconds;
        self
    }

    fn session_key(tenant_id: &str, session_id: &str) -> String {
        format!("pandaria:session:{tenant_id}:{session_id}")
    }

    fn tenant_index_key(tenant_id: &str) -> String {
        format!("pandaria:tenant:{tenant_id}:sessions")
    }
}

#[async_trait]
impl SessionStore for RedisSessionStore {
    async fn save_session(
        &self,
        tenant_id: &str,
        session_id: &str,
        entries: &[SessionEntry],
    ) -> Result<(), AgentError> {
        let key = Self::session_key(tenant_id, session_id);
        let index_key = Self::tenant_index_key(tenant_id);
        let json =
            serde_json::to_string(entries).map_err(|e| {
                AgentError::Persistence(format!("serialize: {e}"))
            })?;

        let mut conn = self.conn.clone();
        let _: () = conn.set(&key, &json).await.map_err(|e| {
            AgentError::Persistence(format!("redis save: {e}"))
        })?;

        // Set TTL to prevent unbounded accumulation
        let _: () = conn.expire(&key, self.ttl_seconds as i64).await.map_err(|e| {
            AgentError::Persistence(format!("redis expire: {e}"))
        })?;

        // Track session in tenant index
        let _: () = conn.sadd(&index_key, session_id).await.map_err(|e| {
            AgentError::Persistence(format!("redis index: {e}"))
        })?;

        debug!(
            tenant_id = %tenant_id,
            session_id = %session_id,
            entry_count = entries.len(),
            "session saved to redis"
        );
        Ok(())
    }

    async fn load_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<Vec<SessionEntry>, AgentError> {
        let key = Self::session_key(tenant_id, session_id);

        let mut conn = self.conn.clone();
        let json: Option<String> = conn.get(&key).await.map_err(|e| {
            AgentError::Persistence(format!("redis load: {e}"))
        })?;

        match json {
            Some(data) => {
                let entries: Vec<SessionEntry> = serde_json::from_str(&data)
                    .map_err(|e| {
                        AgentError::Persistence(format!("deserialize: {e}"))
                    })?;
                debug!(
                    tenant_id = %tenant_id,
                    session_id = %session_id,
                    entry_count = entries.len(),
                    "session loaded from redis"
                );
                Ok(entries)
            }
            None => Ok(Vec::new()),
        }
    }

    async fn delete_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<(), AgentError> {
        let key = Self::session_key(tenant_id, session_id);
        let index_key = Self::tenant_index_key(tenant_id);

        let mut conn = self.conn.clone();
        let _: () = conn.del(&key).await.map_err(|e| {
            AgentError::Persistence(format!("redis delete: {e}"))
        })?;

        let _: () = conn.srem(&index_key, session_id).await.map_err(|e| {
            AgentError::Persistence(format!("redis index remove: {e}"))
        })?;

        Ok(())
    }

    async fn list_sessions(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<String>, AgentError> {
        let index_key = Self::tenant_index_key(tenant_id);

        let mut conn = self.conn.clone();
        let sessions: Vec<String> = conn.smembers(&index_key).await.map_err(|e| {
            AgentError::Persistence(format!("redis list: {e}"))
        })?;

        Ok(sessions)
    }
}
