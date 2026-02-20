-- Create table for dataplane definitions
-- Migration: 20260126000001_create_dataplanes_table.sql
-- Purpose: Store dataplane (Envoy instance) definitions with gateway_host for MCP tool execution

CREATE TABLE IF NOT EXISTS dataplanes (
    id TEXT PRIMARY KEY,
    team TEXT NOT NULL,                          -- Team name for isolation
    name TEXT NOT NULL,                          -- Human-readable dataplane name
    gateway_host TEXT,                           -- Host address for gateway API execution
    description TEXT,                            -- Optional description
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Foreign keys
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE CASCADE,

    -- Dataplane names must be unique per team
    UNIQUE(team, name)
);

-- Indexes for query performance
CREATE INDEX IF NOT EXISTS idx_dataplanes_team ON dataplanes(team);
CREATE INDEX IF NOT EXISTS idx_dataplanes_name ON dataplanes(team, name);

-- Seed dataplanes scopes
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled, created_at, updated_at)
VALUES
    (
        'scope-dataplanes-read',
        'dataplanes:read',
        'dataplanes',
        'read',
        'Read Dataplanes',
        'List and view dataplane configurations',
        'Configuration',
        TRUE,
        TRUE,
        CURRENT_TIMESTAMP,
        CURRENT_TIMESTAMP
    ),
    (
        'scope-dataplanes-write',
        'dataplanes:write',
        'dataplanes',
        'write',
        'Manage Dataplanes',
        'Create, update, and delete dataplane configurations',
        'Configuration',
        TRUE,
        TRUE,
        CURRENT_TIMESTAMP,
        CURRENT_TIMESTAMP
    )
ON CONFLICT (value) DO NOTHING;
