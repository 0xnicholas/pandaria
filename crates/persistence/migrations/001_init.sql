-- pandaria sessions table
-- Migration: 001_init.sql

CREATE TABLE IF NOT EXISTS sessions (
    tenant_id   TEXT NOT NULL,
    session_id  TEXT NOT NULL,
    entries     JSONB NOT NULL DEFAULT '[]',
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, session_id)
);

-- Index for listing sessions by tenant
CREATE INDEX IF NOT EXISTS idx_sessions_tenant ON sessions (tenant_id, updated_at DESC);
