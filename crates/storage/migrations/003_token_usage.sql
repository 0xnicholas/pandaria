-- Per-turn token consumption tracking
-- Migration: 003_token_usage.sql

CREATE TABLE IF NOT EXISTS session_token_usage (
    id             BIGSERIAL PRIMARY KEY,
    tenant_id      TEXT NOT NULL,
    session_id     TEXT NOT NULL,
    turn_number    INTEGER NOT NULL,
    input_tokens   BIGINT NOT NULL DEFAULT 0,
    output_tokens  BIGINT NOT NULL DEFAULT 0,
    model          VARCHAR(128),
    provider       VARCHAR(64),
    recorded_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_token_usage_tenant
    ON session_token_usage (tenant_id, recorded_at DESC);

CREATE INDEX IF NOT EXISTS idx_token_usage_session
    ON session_token_usage (tenant_id, session_id);

-- Sliding-window usage counters (monthly tokens, concurrent sessions, daily tool calls)
CREATE TABLE IF NOT EXISTS tenant_usage_counters (
    tenant_id    TEXT NOT NULL,
    metric       VARCHAR(64) NOT NULL,
    window_start TIMESTAMPTZ NOT NULL,
    value        BIGINT NOT NULL DEFAULT 0,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, metric, window_start)
);
