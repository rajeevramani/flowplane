-- Migration: Add API tools scopes
--
-- These scopes enable fine-grained access control for the new
-- /api/v1/mcp/api endpoint that exposes gateway API tools
-- separately from control plane (CP) tools.
--
-- API tools allow AI assistants to call upstream services
-- through the Envoy gateway, while CP tools are for inspecting
-- gateway configuration.
--
-- Scopes added:
-- - api:read - List available API tools
-- - api:execute - Execute API tools (call upstream APIs)

INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled, created_at, updated_at)
VALUES
    (
        'scope-api-read',
        'api:read',
        'api',
        'read',
        'List API Tools',
        'List available API tools (gateway tools for calling upstream services)',
        'API Gateway',
        TRUE,
        TRUE,
        CURRENT_TIMESTAMP,
        CURRENT_TIMESTAMP
    ),
    (
        'scope-api-execute',
        'api:execute',
        'api',
        'execute',
        'Execute API Tools',
        'Execute API tools to call upstream services through the gateway',
        'API Gateway',
        TRUE,
        TRUE,
        CURRENT_TIMESTAMP,
        CURRENT_TIMESTAMP
    )
ON CONFLICT (value) DO NOTHING;
