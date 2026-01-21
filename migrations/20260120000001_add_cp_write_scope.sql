-- Add cp:write scope for Control Plane write operations via MCP
-- This scope allows creating, updating, and deleting control plane resources
-- (clusters, routes, listeners, filters) through MCP tools.

INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled, created_at, updated_at)
VALUES (
    'scope-cp-write',
    'cp:write',
    'cp',
    'write',
    'Write Control Plane',
    'Create, update, and delete control plane resources (clusters, routes, listeners, filters) via MCP tools',
    'MCP',
    TRUE,
    TRUE,
    CURRENT_TIMESTAMP,
    CURRENT_TIMESTAMP
)
ON CONFLICT (value) DO NOTHING;
