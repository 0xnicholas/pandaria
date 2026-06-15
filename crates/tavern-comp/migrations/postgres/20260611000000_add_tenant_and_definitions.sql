-- Add tenant_id, inputs, context to workflow_instances
ALTER TABLE workflow_instances
    ADD COLUMN IF NOT EXISTS tenant_id TEXT NOT NULL DEFAULT 'default';

ALTER TABLE workflow_instances
    ADD COLUMN IF NOT EXISTS inputs JSONB NOT NULL DEFAULT '{}';

ALTER TABLE workflow_instances
    ADD COLUMN IF NOT EXISTS context JSONB NOT NULL DEFAULT '{}';

-- Replace status-only index with tenant-aware composite index
DROP INDEX IF EXISTS idx_instances_status;
CREATE INDEX IF NOT EXISTS idx_instances_tenant_status
    ON workflow_instances (tenant_id, status);

-- Add event_type and step_id to workflow_events (NULL = pre-migration row)
ALTER TABLE workflow_events
    ADD COLUMN IF NOT EXISTS event_type VARCHAR(64);

ALTER TABLE workflow_events
    ADD COLUMN IF NOT EXISTS step_id VARCHAR(64);

CREATE INDEX IF NOT EXISTS idx_events_type
    ON workflow_events (instance_id, event_type);
CREATE INDEX IF NOT EXISTS idx_events_step
    ON workflow_events (instance_id, step_id) WHERE step_id IS NOT NULL;

-- Agent definitions table (source of truth for API-registered agents; cache for YAML agents)
CREATE TABLE IF NOT EXISTS agent_definitions (
    id                  VARCHAR(64) PRIMARY KEY,
    tenant_id           TEXT NOT NULL DEFAULT 'default',
    name                VARCHAR(128) NOT NULL,
    description         TEXT,
    model_provider      VARCHAR(64) NOT NULL,
    model_name          VARCHAR(128) NOT NULL,
    model_temperature   REAL NOT NULL DEFAULT 0.7,
    instructions        TEXT NOT NULL,
    skills              JSONB NOT NULL DEFAULT '[]',
    constraints         JSONB NOT NULL DEFAULT '[]',
    memory_config       JSONB NOT NULL DEFAULT '{"enabled": false}',
    source              VARCHAR(64) NOT NULL DEFAULT 'yaml',
    version             INTEGER NOT NULL DEFAULT 1,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_agent_defs_tenant
    ON agent_definitions (tenant_id, id);
CREATE INDEX IF NOT EXISTS idx_agent_defs_source
    ON agent_definitions (tenant_id, source);

-- Workflow definitions table (source of truth for API-registered workflows; cache for YAML workflows)
CREATE TABLE IF NOT EXISTS workflow_definitions (
    id              VARCHAR(64) PRIMARY KEY,
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    name            VARCHAR(128) NOT NULL,
    description     TEXT,
    process         VARCHAR(32) NOT NULL DEFAULT 'sequential',
    manager_config  JSONB,
    steps           JSONB NOT NULL,
    inputs          JSONB NOT NULL DEFAULT '[]',
    outputs         JSONB NOT NULL DEFAULT '[]',
    planning_config JSONB,
    webhook_config  JSONB,
    schedule        VARCHAR(64),
    schedule_inputs JSONB DEFAULT '{}',
    source          VARCHAR(64) NOT NULL DEFAULT 'yaml',
    version         INTEGER NOT NULL DEFAULT 1,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_workflow_defs_tenant
    ON workflow_definitions (tenant_id, id);
CREATE INDEX IF NOT EXISTS idx_workflow_defs_source
    ON workflow_definitions (tenant_id, source);
