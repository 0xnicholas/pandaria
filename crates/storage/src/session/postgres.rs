use async_trait::async_trait;
use sqlx::PgPool;
use tracing::info;

use agent_core::{AgentError, SessionEntry, SessionStore};

use crate::StorageError;

/// PostgreSQL implementation of [`SessionStore`].
///
/// Stores session history as JSONB for flexibility, with a single row
/// per (tenant_id, session_id) primary key.
#[derive(Debug, Clone)]
pub struct PgSessionStore {
    pool: PgPool,
}

impl PgSessionStore {
    /// Create a new store backed by the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Run all pending migrations.
    ///
    /// Idempotent — safe to call on every startup. Uses sqlx's embedded
    /// migration runner so the SQL lives in `migrations/*.sql` files,
    /// not in Rust source.
    pub async fn init(&self) -> Result<(), StorageError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| StorageError::Migration(e.to_string()))?;

        info!("pg session store initialised");
        Ok(())
    }

    /// List all session IDs for a given tenant (internal helper).
    async fn list_sessions_inner(&self, tenant_id: &str) -> Result<Vec<String>, StorageError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT session_id FROM sessions WHERE tenant_id = $1 ORDER BY updated_at DESC",
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(sid,)| sid).collect())
    }

    /// Delete a specific session (internal helper).
    async fn delete_session_inner(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM sessions WHERE tenant_id = $1 AND session_id = $2")
            .bind(tenant_id)
            .bind(session_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Update the lifecycle status of a session.
    pub async fn update_status(
        &self,
        tenant_id: &str,
        session_id: &str,
        status: &str,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE sessions SET status = $1, updated_at = NOW() WHERE tenant_id = $2 AND session_id = $3",
        )
        .bind(status)
        .bind(tenant_id)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[async_trait]
impl SessionStore for PgSessionStore {
    async fn save_session(
        &self,
        tenant_id: &str,
        session_id: &str,
        entries: &[SessionEntry],
    ) -> Result<(), AgentError> {
        let json = serde_json::to_value(entries)
            .map_err(|e| AgentError::Persistence(format!("serialize: {e}")))?;

        sqlx::query(
            r#"
            INSERT INTO sessions (tenant_id, session_id, entries, updated_at)
            VALUES ($1, $2, $3, NOW())
            ON CONFLICT (tenant_id, session_id)
            DO UPDATE SET entries = EXCLUDED.entries, updated_at = NOW()
            "#,
        )
        .bind(tenant_id)
        .bind(session_id)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Persistence(format!("pg save: {e}")))?;

        Ok(())
    }

    async fn load_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<Vec<SessionEntry>, AgentError> {
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT entries FROM sessions WHERE tenant_id = $1 AND session_id = $2")
                .bind(tenant_id)
                .bind(session_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| AgentError::Persistence(format!("pg load: {e}")))?;

        match row {
            Some((json,)) => {
                let entries: Vec<SessionEntry> = serde_json::from_value(json)
                    .map_err(|e| AgentError::Persistence(format!("deserialize: {e}")))?;
                Ok(entries)
            }
            None => Ok(Vec::new()),
        }
    }

    async fn delete_session(&self, tenant_id: &str, session_id: &str) -> Result<(), AgentError> {
        self.delete_session_inner(tenant_id, session_id)
            .await
            .map_err(|e| AgentError::Persistence(e.to_string()))
    }

    async fn list_sessions(&self, tenant_id: &str) -> Result<Vec<String>, AgentError> {
        self.list_sessions_inner(tenant_id)
            .await
            .map_err(|e| AgentError::Persistence(e.to_string()))
    }

    async fn append_entries(
        &self,
        tenant_id: &str,
        session_id: &str,
        new_entries: &[SessionEntry],
    ) -> Result<(), AgentError> {
        let json = serde_json::to_value(new_entries)
            .map_err(|e| AgentError::Persistence(format!("serialize: {e}")))?;

        // jsonb || jsonb concatenates the new array onto the existing one
        sqlx::query(
            r#"
            INSERT INTO sessions (tenant_id, session_id, entries, updated_at)
            VALUES ($1, $2, $3, NOW())
            ON CONFLICT (tenant_id, session_id)
            DO UPDATE SET
                entries = sessions.entries || EXCLUDED.entries,
                updated_at = NOW()
            "#,
        )
        .bind(tenant_id)
        .bind(session_id)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Persistence(format!("pg append: {e}")))?;

        Ok(())
    }

    async fn update_session_status(
        &self,
        tenant_id: &str,
        session_id: &str,
        status: &str,
    ) -> Result<(), AgentError> {
        self.update_status(tenant_id, session_id, status)
            .await
            .map_err(|e| AgentError::Persistence(e.to_string()))
    }

    async fn cleanup_expired_sessions(
        &self,
        older_than: std::time::Duration,
    ) -> Result<u64, AgentError> {
        // Use NOW() - interval to allow index usage on updated_at.
        // The interval is computed from older_than in seconds.
        let interval_secs = older_than.as_secs() as i64;
        let interval_str = format!("{} seconds", interval_secs);

        let result = sqlx::query(
            "DELETE FROM sessions WHERE status IN ('completed', 'failed') AND updated_at < NOW() - $1::INTERVAL",
        )
        .bind(&interval_str)
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Persistence(format!("pg cleanup: {e}")))?;

        Ok(result.rows_affected())
    }
}
