-- SQLite: Add tenant_id, inputs, context to workflow_instances
-- SQLite does not support ADD COLUMN IF NOT EXISTS; use bare ADD COLUMN
ALTER TABLE workflow_instances ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'default';
ALTER TABLE workflow_instances ADD COLUMN inputs TEXT NOT NULL DEFAULT '{}';
ALTER TABLE workflow_instances ADD COLUMN context TEXT NOT NULL DEFAULT '{}';

-- Replace status-only index with tenant-aware composite index
DROP INDEX IF EXISTS idx_instances_status;
CREATE INDEX IF NOT EXISTS idx_instances_tenant_status
    ON workflow_instances (tenant_id, status);

-- Add event_type and step_id to workflow_events
ALTER TABLE workflow_events ADD COLUMN event_type TEXT;
ALTER TABLE workflow_events ADD COLUMN step_id TEXT;

CREATE INDEX IF NOT EXISTS idx_events_type
    ON workflow_events (instance_id, event_type);
-- SQLite partial index uses same syntax as PG
CREATE INDEX IF NOT EXISTS idx_events_step
    ON workflow_events (instance_id, step_id) WHERE step_id IS NOT NULL;

-- Agent definitions table
CREATE TABLE IF NOT EXISTS agent_definitions (
    id               TEXT PRIMARY KEY,
    tenant_id        TEXT NOT NULL DEFAULT 'default',
    name             TEXT NOT NULL,
    description      TEXT,
    model_provider   TEXT NOT NULL,
    model_name       TEXT NOT NULL,
    model_temperature REAL NOT NULL DEFAULT 0.7,
    instructions     TEXT NOT NULL,
    skills           TEXT NOT NULL DEFAULT '[]',
    constraints      TEXT NOT NULL DEFAULT '[]',
    memory_config    TEXT NOT NULL DEFAULT '{"enabled": false}',
    source           TEXT NOT NULL DEFAULT 'yaml',
    version          INTEGER NOT NULL DEFAULT 1,
    created_at       INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000),
    updated_at       INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000)
);

CREATE INDEX IF NOT EXISTS idx_agent_defs_tenant
    ON agent_definitions (tenant_id, id);
CREATE INDEX IF NOT EXISTS idx_agent_defs_source
    ON agent_definitions (tenant_id, source);

-- Workflow definitions table
CREATE TABLE IF NOT EXISTS workflow_definitions (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    name            TEXT NOT NULL,
    description     TEXT,
    process         TEXT NOT NULL DEFAULT 'sequential',
    manager_config  TEXT,
    steps           TEXT NOT NULL,
    inputs          TEXT NOT NULL DEFAULT '[]',
    outputs         TEXT NOT NULL DEFAULT '[]',
    planning_config TEXT,
    webhook_config  TEXT,
    schedule        TEXT,
    schedule_inputs TEXT DEFAULT '{}',
    source          TEXT NOT NULL DEFAULT 'yaml',
    version         INTEGER NOT NULL DEFAULT 1,
    created_at      INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000),
    updated_at      INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000)
);

CREATE INDEX IF NOT EXISTS idx_workflow_defs_tenant
    ON workflow_definitions (tenant_id, id);
CREATE INDEX IF NOT EXISTS idx_workflow_defs_source
    ON workflow_definitions (tenant_id, source);
