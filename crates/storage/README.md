# storage

General-purpose storage layer for pandaria.

## Overview

Provides storage adapters for multiple domains. Currently implements the
`agent_core::SessionStore` trait for session history persistence, enabling
session data to survive process restarts and node migrations.

## Session Adapters

| Adapter | Module | Status |
|---|---|---|
| `PgSessionStore` | `session::postgres` | Production-ready |
| `RedisSessionStore` | `session::redis` | Available |

## Usage

### PostgreSQL

```rust
use storage::session::postgres::PgSessionStore;
use sqlx::PgPool;

let pool = PgPool::connect("postgres://user@localhost/pandaria").await?;
let store = PgSessionStore::new(pool.clone());
store.init().await?;
store.save_session("tenant_1", "session_1", &entries).await?;
```

### Redis

```rust
use storage::session::redis::RedisSessionStore;
use redis::aio::ConnectionManager;

let client = redis::Client::open("redis://127.0.0.1/")?;
let conn = ConnectionManager::new(client).await?;
let store = RedisSessionStore::new(conn);
store.save_session("tenant_1", "session_1", &entries).await?;
```

## Schema

See `migrations/001_init.sql` for the PostgreSQL DDL.

## Error types

See `StorageError` in `lib.rs` for the unified error enum covering
all storage backends.

## Architecture

```
storage
  ├── lib.rs                — StorageError (unified error type), module re-exports
  └── session/
      ├── mod.rs             — Session domain re-exports
      ├── postgres.rs        — PgSessionStore (sqlx)
      └── redis.rs           — RedisSessionStore (redis)
```
