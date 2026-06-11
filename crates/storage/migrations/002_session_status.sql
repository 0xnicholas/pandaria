-- Add session lifecycle status and metadata columns
-- Migration: 002_session_status.sql

ALTER TABLE sessions ADD COLUMN IF NOT EXISTS status VARCHAR(16) NOT NULL DEFAULT 'active';
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS metadata JSONB NOT NULL DEFAULT '{}';

-- Index for filtering sessions by status
CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions (tenant_id, status);
