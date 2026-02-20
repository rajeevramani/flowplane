-- Create table for MCP tool definitions
-- Migration: 20260109000004_create_mcp_tools_table.sql
-- Purpose: Store MCP tool definitions with metadata for AI assistant access

CREATE TABLE IF NOT EXISTS mcp_tools (
    id TEXT PRIMARY KEY,
    team TEXT NOT NULL,                          -- Team name for isolation
    name TEXT NOT NULL,                          -- Tool name: "api_getUser"
    description TEXT,
    category TEXT NOT NULL CHECK (category IN ('control_plane', 'gateway_api')),
    source_type TEXT NOT NULL CHECK (source_type IN ('builtin', 'openapi', 'learned', 'manual')),

    -- Schemas
    input_schema TEXT NOT NULL,                  -- JSON Schema: what AI sends
    output_schema TEXT,                          -- JSON Schema: what API returns
    learned_schema_id INTEGER,
    schema_source TEXT CHECK (schema_source IN ('openapi', 'learned', 'manual', 'mixed')),

    -- Gateway metadata (for gateway_api tools only)
    route_id TEXT,                               -- FK to routes table
    http_method TEXT,                            -- GET, POST, PUT, DELETE
    http_path TEXT,                              -- Full path pattern
    cluster_name TEXT,                           -- Target cluster
    listener_port BIGINT,                        -- Envoy listener port for execution

    -- State
    enabled INTEGER DEFAULT 1 CHECK (enabled IN (0, 1)),
    confidence DOUBLE PRECISION,                             -- 1.0 for OpenAPI, 0.0-1.0 for learned
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Foreign keys
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE CASCADE,
    FOREIGN KEY (route_id) REFERENCES routes(id) ON DELETE CASCADE,
    FOREIGN KEY (learned_schema_id) REFERENCES aggregated_api_schemas(id) ON DELETE SET NULL,

    -- Tool names must be unique per team
    UNIQUE(team, name)
);

-- Indexes for query performance
CREATE INDEX IF NOT EXISTS idx_mcp_tools_team ON mcp_tools(team);
CREATE INDEX IF NOT EXISTS idx_mcp_tools_route_id ON mcp_tools(route_id);
CREATE INDEX IF NOT EXISTS idx_mcp_tools_enabled ON mcp_tools(team, enabled);
CREATE INDEX IF NOT EXISTS idx_mcp_tools_category ON mcp_tools(team, category);
CREATE INDEX IF NOT EXISTS idx_mcp_tools_name ON mcp_tools(team, name);

-- Seed MCP-related scopes
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled, created_at, updated_at)
VALUES
    (
        'scope-cp-read',
        'cp:read',
        'cp',
        'read',
        'Read Control Plane',
        'Read control plane configuration (clusters, routes, listeners)',
        'MCP',
        TRUE,
        TRUE,
        CURRENT_TIMESTAMP,
        CURRENT_TIMESTAMP
    ),
    (
        'scope-mcp-read',
        'mcp:read',
        'mcp',
        'read',
        'List MCP Tools',
        'List available MCP gateway tools',
        'MCP',
        TRUE,
        TRUE,
        CURRENT_TIMESTAMP,
        CURRENT_TIMESTAMP
    ),
    (
        'scope-mcp-execute',
        'mcp:execute',
        'mcp',
        'execute',
        'Execute MCP Tools',
        'Execute MCP gateway tools (call upstream APIs)',
        'MCP',
        TRUE,
        TRUE,
        CURRENT_TIMESTAMP,
        CURRENT_TIMESTAMP
    ),
    (
        'scope-mcp-write',
        'mcp:write',
        'mcp',
        'write',
        'Manage MCP Tools',
        'Enable/disable MCP on routes, manage tool configuration',
        'MCP',
        TRUE,
        TRUE,
        CURRENT_TIMESTAMP,
        CURRENT_TIMESTAMP
    )
ON CONFLICT (value) DO NOTHING;
