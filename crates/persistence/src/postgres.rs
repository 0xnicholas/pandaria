use async_trait::async_trait;
use sqlx::PgPool;
use tracing::info;

use agent_core::{AgentError, SessionEntry, SessionStore};

use crate::error::PersistenceError;

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

    /// Initialise the `sessions` table if it does not already exist.
    pub async fn init(&self) -> Result<(), PersistenceError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                tenant_id   TEXT NOT NULL,
                session_id  TEXT NOT NULL,
                entries     JSONB NOT NULL DEFAULT '[]',
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                PRIMARY KEY (tenant_id, session_id)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Idempotent index creation that survives concurrent init() calls
        sqlx::query(
            r#"
            DO $$
            BEGIN
                IF NOT EXISTS (
                    SELECT 1 FROM pg_indexes
                    WHERE indexname = 'idx_sessions_tenant'
                ) THEN
                    CREATE INDEX idx_sessions_tenant ON sessions (tenant_id, updated_at DESC);
                END IF;
            END
            $$
            "#,
        )
        .execute(&self.pool)
        .await?;

        info!("pg session store initialised");
        Ok(())
    }

    /// List all session IDs for a given tenant (internal helper).
    async fn list_sessions_inner(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<String>, PersistenceError> {
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
    ) -> Result<(), PersistenceError> {
        sqlx::query("DELETE FROM sessions WHERE tenant_id = $1 AND session_id = $2")
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
        let row: Option<(serde_json::Value,)> = sqlx::query_as(
            "SELECT entries FROM sessions WHERE tenant_id = $1 AND session_id = $2",
        )
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

    async fn delete_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<(), AgentError> {
        self.delete_session_inner(tenant_id, session_id)
            .await
            .map_err(|e| AgentError::Persistence(e.to_string()))
    }

    async fn list_sessions(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<String>, AgentError> {
        self.list_sessions_inner(tenant_id)
            .await
            .map_err(|e| AgentError::Persistence(e.to_string()))
    }
}
