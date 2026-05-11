# persistence

Session history persistence layer for pandaria.

## Overview

Provides storage adapters that implement the `agent_core::SessionStore` trait,
enabling session history to survive process restarts and node migrations.

## Adapters

| Adapter | Module | Status |
|---|---|---|
| `PgSessionStore` | `postgres` | Production-ready |
| `RedisSessionStore` | `redis_store` | Available |

## Usage

### PostgreSQL

```rust
use persistence::postgres::PgSessionStore;
use sqlx::PgPool;

let pool = PgPool::connect("postgres://user@localhost/pandaria").await?;
let store = PgSessionStore::new(pool.clone());
store.init().await?;
store.save_session("tenant_1", "session_1", &entries).await?;
```

### Redis

```rust
use persistence::redis_store::RedisSessionStore;
use redis::aio::ConnectionManager;

let client = redis::Client::open("redis://127.0.0.1/")?;
let conn = ConnectionManager::new(client).await?;
let store = RedisSessionStore::new(conn);
store.save_session("tenant_1", "session_1", &entries).await?;
```

## Schema

See `migrations/001_init.sql` for the PostgreSQL DDL.

## Error types

See `error::PersistenceError` for structured error variants covering
both PostgreSQL and Redis backends.

## Architecture

```
persistence
  ├── error.rs          — PersistenceError (thiserror)
  ├── postgres.rs        — PgSessionStore (sqlx)
  ├── redis_store.rs     — RedisSessionStore (redis)
  └── lib.rs             — module re-exports
```
