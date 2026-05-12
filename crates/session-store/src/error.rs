/// Persistence-specific errors.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[cfg(feature = "postgres")]
    #[error("postgres error: {0}")]
    Postgres(#[from] sqlx::Error),

    #[cfg(feature = "redis")]
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("migration error: {0}")]
    Migration(String),

    #[error("store not initialized — call init() first")]
    NotInitialized,
}
